use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use super::tokens::count_tokens;

/// Process-global monotonic base for encoding `Instant`s into an `AtomicU64`.
/// Stored as milliseconds since this base, which is sufficient resolution for
/// LRU/RRF eviction recency while allowing lock-free access on cache hits.
fn instant_base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

fn encode_instant(i: Instant) -> u64 {
    i.saturating_duration_since(instant_base()).as_millis() as u64
}

fn decode_instant(ms: u64) -> Instant {
    instant_base() + Duration::from_millis(ms)
}

fn normalize_key(path: &str) -> String {
    crate::core::pathutil::normalize_tool_path(path)
}

/// Built-in token budget for the in-memory read cache: several hundred
/// typical source files. RAM-pressure eviction via EvictionOrchestrator
/// provides an independent safety net regardless of this budget.
/// Shared with `eviction_orchestrator` so both eviction rails read one budget.
pub(crate) const CACHE_MAX_TOKENS: usize = 2_000_000;

/// A cached file read: zstd-compressed content, hash, token count, and access metadata.
///
/// `read_count` and `last_access` use interior mutability (atomics) so cache
/// hits can be recorded under a shared (read) lock — parallel reads of distinct
/// files do not serialize on a global write lock.
#[derive(Debug)]
pub struct CacheEntry {
    compressed_content: Vec<u8>,
    pub hash: String,
    pub line_count: usize,
    pub original_tokens: usize,
    read_count: AtomicU32,
    last_access: AtomicU64,
    pub stored_mtime: Option<SystemTime>,
    /// Mode-specific compressed outputs (e.g. "map", "signatures") cached to avoid re-parsing.
    pub compressed_outputs: HashMap<String, String>,
    /// Whether full (uncompressed) content was already delivered for this hash.
    /// Prevents cache-stub loops when upgrading from compressed to full mode.
    pub full_content_delivered: bool,
    /// Conversation id that received the full content (see
    /// [`crate::core::conversation`]). The `[unchanged]` stub is only valid for
    /// a re-read from this same conversation; `None` means delivered without a
    /// known conversation context (hooks absent).
    pub delivered_conversation: Option<String>,
    /// Last read mode used for this file (for auto-escalation on edit failure).
    pub last_mode: String,
}

const ZSTD_LEVEL: i32 = 3;

fn zstd_compress(data: &str) -> Vec<u8> {
    zstd::encode_all(data.as_bytes(), ZSTD_LEVEL).unwrap_or_else(|_| data.as_bytes().to_vec())
}

fn zstd_decompress(data: &[u8]) -> Option<String> {
    zstd::decode_all(data)
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
}

impl CacheEntry {
    /// Creates a new entry with zstd-compressed content.
    pub fn new(
        content: &str,
        hash: String,
        line_count: usize,
        original_tokens: usize,
        stored_mtime: Option<SystemTime>,
    ) -> Self {
        let compressed_content = zstd_compress(content);
        Self {
            compressed_content,
            hash,
            line_count,
            original_tokens,
            read_count: AtomicU32::new(1),
            last_access: AtomicU64::new(encode_instant(Instant::now())),
            stored_mtime,
            compressed_outputs: HashMap::new(),
            full_content_delivered: false,
            delivered_conversation: None,
            last_mode: String::new(),
        }
    }

    /// Current read count (lock-free).
    pub fn read_count(&self) -> u32 {
        self.read_count.load(Ordering::Relaxed)
    }

    /// Atomically increments the read count and returns the new value (lock-free).
    pub fn bump_read_count(&self) {
        self.read_count.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    /// Overwrites the read count (used by `store` and tests).
    pub fn set_read_count(&self, n: u32) {
        self.read_count.store(n, Ordering::Relaxed);
    }

    /// Last access time, decoded from the atomic millisecond offset.
    pub fn last_access(&self) -> Instant {
        decode_instant(self.last_access.load(Ordering::Relaxed))
    }

    /// Marks the entry as accessed now (lock-free).
    pub fn touch(&self) {
        self.last_access
            .store(encode_instant(Instant::now()), Ordering::Relaxed);
    }

    /// Overwrites the last-access time (used by tests and eviction setup).
    pub fn set_last_access(&self, when: Instant) {
        self.last_access
            .store(encode_instant(when), Ordering::Relaxed);
    }

    /// Decompresses and returns the full file content.
    pub fn content(&self) -> Option<String> {
        zstd_decompress(&self.compressed_content)
    }

    /// Replaces the stored content with new zstd-compressed data.
    pub fn set_content(&mut self, content: &str) {
        self.compressed_content = zstd_compress(content);
    }
}

/// Result of a cache store operation, indicating whether it was a hit or new entry.
#[derive(Debug, Clone)]
pub struct StoreResult {
    pub line_count: usize,
    pub original_tokens: usize,
    pub was_hit: bool,
}

impl CacheEntry {
    pub fn get_compressed(&self, mode_key: &str) -> Option<&String> {
        self.compressed_outputs.get(mode_key)
    }

    pub fn set_compressed(&mut self, mode_key: &str, output: String) {
        const MAX_COMPRESSED_VARIANTS: usize = 3;
        if self.compressed_outputs.len() >= MAX_COMPRESSED_VARIANTS
            && !self.compressed_outputs.contains_key(mode_key)
            && let Some(oldest_key) = self.compressed_outputs.keys().next().cloned()
        {
            self.compressed_outputs.remove(&oldest_key);
        }
        self.compressed_outputs.insert(mode_key.to_string(), output);
    }

    pub fn mark_full_delivered(&mut self, conversation: Option<String>) {
        self.full_content_delivered = true;
        self.delivered_conversation = conversation;
    }
}

const RRF_K: f64 = 60.0;

/// Compute Reciprocal Rank Fusion eviction scores for a batch of cache entries.
/// Each signal (recency, frequency, size) produces an independent ranking.
/// The final score is the sum of `1/(k + rank)` across all signals.
/// Higher score = more valuable = keep longer.
pub fn eviction_scores_rrf(entries: &[(&String, &CacheEntry)], now: Instant) -> Vec<(String, f64)> {
    if entries.is_empty() {
        return Vec::new();
    }

    let n = entries.len();

    let mut recency_order: Vec<usize> = (0..n).collect();
    recency_order.sort_by(|&a, &b| {
        let elapsed_a = now
            .checked_duration_since(entries[a].1.last_access())
            .unwrap_or_default()
            .as_secs_f64();
        let elapsed_b = now
            .checked_duration_since(entries[b].1.last_access())
            .unwrap_or_default()
            .as_secs_f64();
        elapsed_a
            .partial_cmp(&elapsed_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut frequency_order: Vec<usize> = (0..n).collect();
    frequency_order.sort_by(|&a, &b| entries[b].1.read_count().cmp(&entries[a].1.read_count()));

    let mut size_order: Vec<usize> = (0..n).collect();
    size_order.sort_by(|&a, &b| {
        entries[b]
            .1
            .original_tokens
            .cmp(&entries[a].1.original_tokens)
    });

    let mut recency_ranks = vec![0usize; n];
    let mut frequency_ranks = vec![0usize; n];
    let mut size_ranks = vec![0usize; n];

    for (rank, &idx) in recency_order.iter().enumerate() {
        recency_ranks[idx] = rank;
    }
    for (rank, &idx) in frequency_order.iter().enumerate() {
        frequency_ranks[idx] = rank;
    }
    for (rank, &idx) in size_order.iter().enumerate() {
        size_ranks[idx] = rank;
    }

    entries
        .iter()
        .enumerate()
        .map(|(i, (path, _))| {
            let score = 1.0 / (RRF_K + recency_ranks[i] as f64)
                + 1.0 / (RRF_K + frequency_ranks[i] as f64)
                + 1.0 / (RRF_K + size_ranks[i] as f64);
            ((*path).clone(), score)
        })
        .collect()
}

/// A block shared across multiple files, identified by its canonical source.
#[derive(Clone, Debug)]
pub struct SharedBlock {
    pub canonical_path: String,
    pub canonical_ref: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}

/// In-memory file cache with RRF eviction (recency, frequency, size), file
/// references, and cross-file dedup.
pub struct SessionCache {
    entries: HashMap<String, CacheEntry>,
    file_refs: HashMap<String, String>,
    next_ref: usize,
    shared_blocks: Vec<SharedBlock>,
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCache {
    /// Creates an empty session cache.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            file_refs: HashMap::new(),
            next_ref: 1,
            shared_blocks: Vec::new(),
        }
    }

    /// Returns or assigns a short file reference label (F1, F2, ...) for the given path.
    pub fn get_file_ref(&mut self, path: &str) -> String {
        let key = normalize_key(path);
        if let Some(r) = self.file_refs.get(&key) {
            return r.clone();
        }
        let r = format!("F{}", self.next_ref);
        self.next_ref += 1;
        self.file_refs.insert(key, r.clone());
        r
    }

    /// Returns the file reference label for a path without assigning a new one.
    pub fn get_file_ref_readonly(&self, path: &str) -> Option<String> {
        self.file_refs.get(&normalize_key(path)).cloned()
    }

    /// Looks up a cached entry by file path.
    pub fn get(&self, path: &str) -> Option<&CacheEntry> {
        self.entries.get(&normalize_key(path))
    }

    /// Mutable lookup of a cached entry by file path.
    pub fn get_mut(&mut self, path: &str) -> Option<&mut CacheEntry> {
        self.entries.get_mut(&normalize_key(path))
    }

    /// Records a cache hit: bumps the entry's read count and access time.
    ///
    /// Takes `&self`: both are interior-mutable atomics, so this
    /// runs under a shared (read) lock and lets parallel reads of different
    /// files proceed concurrently instead of serializing on a write lock.
    pub fn record_cache_hit(&self, path: &str) -> Option<&CacheEntry> {
        let entry = self.entries.get(&normalize_key(path))?;
        entry.bump_read_count();
        entry.touch();
        Some(entry)
    }

    /// Stores file content in the cache; returns a hit if content hash matches.
    pub fn store(&mut self, path: &str, content: &str) -> StoreResult {
        let key = normalize_key(path);
        let hash = content_hash(content);
        let line_count = content.lines().count();
        let original_tokens = count_tokens(content);
        let stored_mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let now = Instant::now();

        if let Some(existing) = self.entries.get_mut(&key) {
            existing.set_last_access(now);
            if stored_mtime.is_some() {
                existing.stored_mtime = stored_mtime;
            }
            if existing.hash == hash {
                existing.bump_read_count();
                return StoreResult {
                    line_count: existing.line_count,
                    original_tokens: existing.original_tokens,
                    was_hit: true,
                };
            }
            existing.compressed_outputs.clear();
            existing.set_content(content);
            existing.hash = hash;
            existing.line_count = line_count;
            existing.original_tokens = original_tokens;
            existing.bump_read_count();
            existing.full_content_delivered = false;
            existing.delivered_conversation = None;
            if stored_mtime.is_some() {
                existing.stored_mtime = stored_mtime;
            }
            return StoreResult {
                line_count,
                original_tokens,
                was_hit: false,
            };
        }

        self.evict_if_needed(original_tokens);
        self.get_file_ref(&key);

        let entry = CacheEntry::new(content, hash, line_count, original_tokens, stored_mtime);

        self.entries.insert(key, entry);
        StoreResult {
            line_count,
            original_tokens,
            was_hit: false,
        }
    }

    /// Returns the sum of original token counts across all cached entries.
    pub fn total_cached_tokens(&self) -> usize {
        self.entries.values().map(|e| e.original_tokens).sum()
    }

    /// Evict until cache fits within token budget using RRF (Reciprocal Rank Fusion).
    /// Combines recency, frequency, and size signals to evict least-valuable entries first.
    pub fn evict_if_needed(&mut self, incoming_tokens: usize) {
        let max_tokens = CACHE_MAX_TOKENS;
        let current = self.total_cached_tokens();
        if current + incoming_tokens <= max_tokens {
            return;
        }

        let now = Instant::now();
        let all: Vec<(&String, &CacheEntry)> = self.entries.iter().collect();
        let mut scores = eviction_scores_rrf(&all, now);
        // Sort ascending: lowest RRF score = least valuable = evict first
        scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut freed = 0usize;
        let target = (current + incoming_tokens).saturating_sub(max_tokens);

        for (path, _score) in &scores {
            if freed >= target {
                break;
            }
            if let Some(entry) = self.entries.remove(path) {
                freed += entry.original_tokens;
                self.file_refs.remove(path);
            }
        }
    }

    /// Returns all cached entries as (path, entry) pairs.
    pub fn get_all_entries(&self) -> Vec<(&String, &CacheEntry)> {
        self.entries.iter().collect()
    }

    /// Returns the path-to-file-ref mapping (e.g. "/src/main.rs" → "F1").
    pub fn file_ref_map(&self) -> &HashMap<String, String> {
        &self.file_refs
    }

    /// Replaces the cross-file shared blocks used for deduplication.
    pub fn set_shared_blocks(&mut self, blocks: Vec<SharedBlock>) {
        self.shared_blocks = blocks;
    }

    /// Replace shared blocks in content with cross-file references.
    pub fn apply_dedup(&self, path: &str, content: &str) -> Option<String> {
        if self.shared_blocks.is_empty() {
            return None;
        }
        let refs: Vec<&SharedBlock> = self
            .shared_blocks
            .iter()
            .filter(|b| b.canonical_path != path && content.contains(&b.content))
            .collect();
        if refs.is_empty() {
            return None;
        }
        let mut result = content.to_string();
        for block in refs {
            result = result.replacen(
                &block.content,
                &format!(
                    "[= {}:{}-{}]",
                    block.canonical_ref, block.start_line, block.end_line
                ),
                1,
            );
        }
        Some(result)
    }

    /// Removes a file from the cache, and its delivery record from the
    /// persistent stub index, so the next read returns the content and never
    /// an `[unchanged]` stub.
    pub fn invalidate(&mut self, path: &str) -> bool {
        let key = normalize_key(path);
        crate::core::read_stub_index::forget(&key);
        self.entries.remove(&key).is_some()
    }

    /// Returns a cached compressed output for a given file and mode key.
    /// Counts as a cache hit — the caller avoids a full disk read + recompression.
    pub fn get_compressed(&self, path: &str, mode_key: &str) -> Option<&String> {
        let key = normalize_key(path);
        let entry = self.entries.get(&key)?;
        let result = entry.get_compressed(mode_key)?;
        entry.bump_read_count();
        entry.touch();
        Some(result)
    }

    /// Marks that full (uncompressed) content was delivered for this file,
    /// tagging it with the current conversation so a later re-read only serves
    /// the `[unchanged]` stub to the same conversation (see
    /// [`crate::core::conversation`]).
    pub fn mark_full_delivered(&mut self, path: &str) {
        let conversation = crate::core::conversation::current_conversation_id();
        let key = normalize_key(path);
        let file_ref = self.file_refs.get(&key).cloned();
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.mark_full_delivered(conversation.clone());
            // Write-through to the persistent stub index so an unchanged re-read
            // in the same conversation survives an MCP restart or idle clear.
            // `record` ignores None-conversation deliveries.
            crate::core::read_stub_index::record(crate::core::read_stub_index::StubRecord::new(
                key.clone(),
                entry.hash.clone(),
                entry.stored_mtime,
                entry.line_count,
                file_ref.unwrap_or_default(),
                conversation,
            ));
        }
    }

    /// Stores a compressed output for a given file and mode key.
    pub fn set_compressed(&mut self, path: &str, mode_key: &str, output: String) {
        if let Some(entry) = self.entries.get_mut(&normalize_key(path)) {
            entry.set_compressed(mode_key, output);
        }
    }

    /// Resets `full_content_delivered` for all entries without removing them.
    /// Used after host context compaction — forces re-delivery on next read
    /// while preserving compressed content and file refs.
    pub fn reset_delivery_flags(&mut self) -> usize {
        let mut count = 0;
        for entry in self.entries.values_mut() {
            if entry.full_content_delivered {
                entry.full_content_delivered = false;
                count += 1;
            }
        }
        count
    }

    /// Returns whether full content was previously delivered for this path.
    pub fn is_full_delivered(&self, path: &str) -> bool {
        self.entries
            .get(&normalize_key(path))
            .is_some_and(|e| e.full_content_delivered)
    }

    /// Removes all compressed output variants (map, signatures, etc.) from every entry,
    /// keeping the full zstd-compressed content intact. Returns the number of entries trimmed.
    pub fn trim_compressed_outputs(&mut self) -> usize {
        let mut trimmed = 0;
        for entry in self.entries.values_mut() {
            if !entry.compressed_outputs.is_empty() {
                entry.compressed_outputs.clear();
                trimmed += 1;
            }
        }
        trimmed
    }

    /// Evicts all entries that have been read at most once (probationary).
    /// Returns the number of entries removed.
    pub fn evict_probationary(&mut self) -> usize {
        let to_remove: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.read_count() <= 1)
            .map(|(k, _)| k.clone())
            .collect();
        let count = to_remove.len();
        for key in &to_remove {
            self.entries.remove(key);
            self.file_refs.remove(key);
        }
        count
    }

    const MAX_SHARED_BLOCKS: usize = 100;

    /// Trims shared blocks to a maximum count, keeping the most recent.
    pub fn trim_shared_blocks(&mut self) {
        if self.shared_blocks.len() > Self::MAX_SHARED_BLOCKS {
            let excess = self.shared_blocks.len() - Self::MAX_SHARED_BLOCKS;
            self.shared_blocks.drain(..excess);
        }
    }

    /// Clears all cached entries and file refs. Returns the number of entries removed.
    pub fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        self.file_refs.clear();
        self.shared_blocks.clear();
        self.next_ref = 1;
        count
    }
}

pub fn file_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub fn is_cache_entry_stale(path: &str, cached_mtime: Option<SystemTime>) -> bool {
    let current = file_mtime(path);
    match (cached_mtime, current) {
        // Both unavailable (e.g. WSL DrvFS): can't tell → assume fresh (conservative).
        (None, None) => false,
        // One side missing: metadata changed or appeared/disappeared → stale.
        (Some(_), None) | (None, Some(_)) => true,
        // `!=`, not `>`: a *backward* mtime (git checkout, touch -t, snapshot
        // restore) is just as much a content change as a forward one.
        (Some(cached), Some(current)) => current != cached,
    }
}

/// Files larger than this are not content-hashed for stub verification and
/// always read as stale. A read is capped well below it
/// ([`crate::core::limits::max_read_bytes`]), so only a file that grew past it
/// since it was cached gets here.
const VERIFY_HASH_CAP_BYTES: u64 = 8 * 1024 * 1024;

/// Staleness with content verification: like [`is_cache_entry_stale`], but when
/// the mtime claims "unchanged", additionally compares the hash of the on-disk
/// content against the cached hash.
///
/// mtime alone cannot be trusted for *correctness*: same-second writes are
/// invisible on coarse-granularity filesystems (HFS+ 1s, FAT 2s) and mtimes can
/// be restored by tools. Serving an `[unchanged]` stub for changed content
/// would silently mislead the agent — the worst failure mode a context layer
/// can have. The extra disk read costs microseconds for typical source files;
/// the stub's token savings are unaffected.
///
/// The cache stores the file as read (redaction applies to output only), so
/// an unchanged file hashes the same.
pub fn is_cache_entry_stale_verified(
    path: &str,
    cached_mtime: Option<SystemTime>,
    cached_hash: &str,
) -> bool {
    if is_cache_entry_stale(path, cached_mtime) {
        return true;
    }
    if cached_hash.is_empty() {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        // Can't stat → never serve a stub on top of it.
        return true;
    };
    if meta.len() > VERIFY_HASH_CAP_BYTES {
        // Too large to hash: the content cannot be vouched for.
        return true;
    }
    match std::fs::read(path) {
        // Hash the same view of the bytes that `store()` hashed: lossy UTF-8,
        // without a UTF-8 BOM (`read_file_lossy` strips it before storing).
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            content_hash(text.strip_prefix('\u{feff}').unwrap_or(&text)) != cached_hash
        }
        Err(_) => true,
    }
}

fn content_hash(content: &str) -> String {
    crate::core::hasher::hash_str(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cache_stores_and_retrieves() {
        let mut cache = SessionCache::new();
        let result = cache.store("/test/file.rs", "fn main() {}");
        assert!(!result.was_hit);
        assert_eq!(result.line_count, 1);
        assert!(cache.get("/test/file.rs").is_some());
    }

    #[test]
    fn cache_hit_on_same_content() {
        let mut cache = SessionCache::new();
        cache.store("/test/file.rs", "content");
        let result = cache.store("/test/file.rs", "content");
        assert!(result.was_hit, "same content should be a cache hit");
    }

    #[test]
    fn cache_miss_on_changed_content() {
        let mut cache = SessionCache::new();
        cache.store("/test/file.rs", "old content");
        let result = cache.store("/test/file.rs", "new content");
        assert!(!result.was_hit, "changed content should not be a cache hit");
    }

    #[test]
    fn file_refs_are_sequential() {
        let mut cache = SessionCache::new();
        assert_eq!(cache.get_file_ref("/a.rs"), "F1");
        assert_eq!(cache.get_file_ref("/b.rs"), "F2");
        assert_eq!(cache.get_file_ref("/a.rs"), "F1"); // stable
    }

    #[test]
    fn cache_clear_resets_everything() {
        let mut cache = SessionCache::new();
        cache.store("/a.rs", "a");
        cache.store("/b.rs", "b");
        let count = cache.clear();
        assert_eq!(count, 2);
        assert!(cache.get("/a.rs").is_none());
        assert_eq!(cache.get_file_ref("/c.rs"), "F1"); // refs reset
    }

    #[test]
    fn cache_invalidate_removes_entry() {
        let mut cache = SessionCache::new();
        cache.store("/test.rs", "test");
        assert!(cache.invalidate("/test.rs"));
        assert!(!cache.invalidate("/nonexistent.rs"));
    }

    #[test]
    #[serial_test::serial(stub_index)]
    fn invalidate_drops_the_persisted_delivery_record() {
        crate::core::read_stub_index::clear_for_test();
        let mut cache = SessionCache::new();
        cache.store("/test/sent-then-dropped.rs", "fn a() {}");
        let entry = cache.get("/test/sent-then-dropped.rs").unwrap();
        crate::core::read_stub_index::record(crate::core::read_stub_index::StubRecord::new(
            normalize_key("/test/sent-then-dropped.rs"),
            entry.hash.clone(),
            entry.stored_mtime,
            entry.line_count,
            "F1".to_string(),
            Some("conv-a".to_string()),
        ));
        assert!(crate::core::read_stub_index::lookup("/test/sent-then-dropped.rs").is_some());

        assert!(cache.invalidate("/test/sent-then-dropped.rs"));
        assert!(
            crate::core::read_stub_index::lookup("/test/sent-then-dropped.rs").is_none(),
            "a cold read must not find a record for content that was dropped"
        );
        crate::core::read_stub_index::clear_for_test();
    }

    #[test]
    fn record_cache_hit_works_through_shared_ref() {
        let mut cache = SessionCache::new();
        cache.store("/x.rs", "hello world");
        // &self path: a cache hit can be recorded without a write lock.
        let shared: &SessionCache = &cache;
        assert!(shared.record_cache_hit("/x.rs").is_some());
        assert!(shared.record_cache_hit("/x.rs").is_some());
        // store=1 + two hits => read_count 3.
        assert_eq!(cache.get("/x.rs").unwrap().read_count(), 3);
    }

    #[test]
    fn concurrent_cache_hits_are_lossless() {
        use std::sync::Arc;
        let mut cache = SessionCache::new();
        cache.store("/a.rs", "a");
        cache.store("/b.rs", "b");
        // Shared (no RwLock): proves SessionCache is Sync and hit recording is
        // lock-free and atomic.
        let cache = Arc::new(cache);
        let threads = 8;
        let iters = 1_000;
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let c = Arc::clone(&cache);
                std::thread::spawn(move || {
                    for _ in 0..iters {
                        c.record_cache_hit("/a.rs");
                        c.record_cache_hit("/b.rs");
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let total = threads * iters;
        assert_eq!(cache.get("/a.rs").unwrap().read_count(), 1 + total as u32);
        assert_eq!(cache.get("/b.rs").unwrap().read_count(), 1 + total as u32);
    }

    #[test]
    fn content_hash_is_deterministic() {
        let h1 = content_hash("test content");
        let h2 = content_hash("test content");
        assert_eq!(h1, h2);
        assert_ne!(h1, content_hash("different"));
    }

    #[test]
    fn rrf_eviction_prefers_recent() {
        let key_a = "a.rs".to_string();
        let key_b = "b.rs".to_string();
        // Construct entries first so the global instant base is initialized,
        // then assign access times relative to a post-init reference.
        let recent = CacheEntry::new("a", "h1".to_string(), 1, 10, None);
        let old = CacheEntry::new("b", "h2".to_string(), 1, 10, None);
        let t_old = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t_recent = Instant::now();
        old.set_last_access(t_old);
        recent.set_last_access(t_recent);
        let now = Instant::now();
        let entries: Vec<(&String, &CacheEntry)> = vec![(&key_a, &recent), (&key_b, &old)];
        let scores = eviction_scores_rrf(&entries, now);
        let score_a = scores.iter().find(|(p, _)| p == "a.rs").unwrap().1;
        let score_b = scores.iter().find(|(p, _)| p == "b.rs").unwrap().1;
        assert!(
            score_a > score_b,
            "recently accessed entries should score higher via RRF"
        );
    }

    #[test]
    fn rrf_eviction_prefers_frequent() {
        let now = Instant::now();
        let key_a = "a.rs".to_string();
        let key_b = "b.rs".to_string();
        let frequent = {
            let e = CacheEntry::new("a", "h1".to_string(), 1, 10, None);
            e.set_read_count(20);
            e
        };
        let rare = CacheEntry::new("b", "h2".to_string(), 1, 10, None);
        let entries: Vec<(&String, &CacheEntry)> = vec![(&key_a, &frequent), (&key_b, &rare)];
        let scores = eviction_scores_rrf(&entries, now);
        let score_a = scores.iter().find(|(p, _)| p == "a.rs").unwrap().1;
        let score_b = scores.iter().find(|(p, _)| p == "b.rs").unwrap().1;
        assert!(
            score_a > score_b,
            "frequently accessed entries should score higher via RRF"
        );
    }

    #[test]
    fn stale_detection_flags_newer_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.txt");
        let p = path.to_string_lossy().to_string();

        std::fs::write(&path, "one").unwrap();
        let mut cache = SessionCache::new();
        cache.store(&p, "one");

        let entry = cache.get(&p).unwrap();
        assert!(!is_cache_entry_stale(&p, entry.stored_mtime));
        crate::test_env::write_newer(&path, "two");

        let entry = cache.get(&p).unwrap();
        assert!(is_cache_entry_stale(&p, entry.stored_mtime));
    }

    // A *backward* mtime (git checkout, touch -t) is a change.
    #[test]
    fn stale_detection_flags_backward_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backward.txt");
        let p = path.to_string_lossy().to_string();

        std::fs::write(&path, "one").unwrap();
        let mut cache = SessionCache::new();
        cache.store(&p, "one");
        let entry_mtime = cache.get(&p).unwrap().stored_mtime;
        assert!(!is_cache_entry_stale(&p, entry_mtime));

        // Simulate `git checkout` of an older version: content + older mtime.
        std::fs::write(&path, "zero").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_hours(1))
            .unwrap();
        drop(f);

        assert!(
            is_cache_entry_stale(&p, entry_mtime),
            "older mtime must read as stale"
        );
    }

    // Identical mtime with different content (same-second write,
    // restored timestamps) is caught by the content-hash verification.
    #[test]
    fn verified_staleness_catches_same_mtime_content_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sneaky.txt");
        let p = path.to_string_lossy().to_string();

        std::fs::write(&path, "one").unwrap();
        let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut cache = SessionCache::new();
        cache.store(&p, "one");
        let (mtime, hash) = {
            let e = cache.get(&p).unwrap();
            (e.stored_mtime, e.hash.clone())
        };

        // Unchanged file: both checks agree it is fresh.
        assert!(!is_cache_entry_stale_verified(&p, mtime, &hash));

        // Change the content but restore the exact original mtime.
        std::fs::write(&path, "two").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_modified(original_mtime).unwrap();
        drop(f);

        assert!(
            !is_cache_entry_stale(&p, mtime),
            "test premise: the mtime check alone is fooled"
        );
        assert!(
            is_cache_entry_stale_verified(&p, mtime, &hash),
            "hash verification must catch the change"
        );
    }

    /// A file too large to hash is never vouched for by its mtime alone.
    #[test]
    fn verified_staleness_flags_file_too_large_to_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grown.txt");
        let p = path.to_string_lossy().to_string();

        std::fs::write(&path, "one").unwrap();
        let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut cache = SessionCache::new();
        cache.store(&p, "one");
        let (mtime, hash) = {
            let e = cache.get(&p).unwrap();
            (e.stored_mtime, e.hash.clone())
        };

        let big = vec![b'x'; usize::try_from(VERIFY_HASH_CAP_BYTES).unwrap() + 1];
        std::fs::write(&path, big).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_modified(original_mtime).unwrap();
        drop(f);

        assert!(is_cache_entry_stale_verified(&p, mtime, &hash));
    }

    #[test]
    fn verified_staleness_flags_unreadable_file() {
        let mut cache = SessionCache::new();
        cache.store("/nonexistent/file.rs", "content");
        let (mtime, hash) = {
            let e = cache.get("/nonexistent/file.rs").unwrap();
            (e.stored_mtime, e.hash.clone())
        };
        assert!(is_cache_entry_stale_verified(
            "/nonexistent/file.rs",
            mtime,
            &hash
        ));
    }

    #[test]
    fn compressed_outputs_cached_and_retrieved() {
        let mut cache = SessionCache::new();
        cache.store("/test.rs", "fn main() {}");
        cache.set_compressed("/test.rs", "map", "compressed map output".to_string());
        assert_eq!(
            cache.get_compressed("/test.rs", "map"),
            Some(&"compressed map output".to_string())
        );
        assert_eq!(cache.get_compressed("/test.rs", "signatures"), None);
    }

    #[test]
    fn compressed_outputs_cleared_on_content_change() {
        let mut cache = SessionCache::new();
        cache.store("/test.rs", "old content");
        cache.set_compressed("/test.rs", "map", "old map".to_string());
        assert!(cache.get_compressed("/test.rs", "map").is_some());

        cache.store("/test.rs", "new content");
        assert_eq!(cache.get_compressed("/test.rs", "map"), None);
    }

    #[test]
    fn compressed_outputs_survive_same_content_store() {
        let mut cache = SessionCache::new();
        cache.store("/test.rs", "content");
        cache.set_compressed("/test.rs", "map", "cached map".to_string());

        let result = cache.store("/test.rs", "content");
        assert!(result.was_hit);
        assert_eq!(
            cache.get_compressed("/test.rs", "map"),
            Some(&"cached map".to_string())
        );
    }

    #[test]
    fn compressed_outputs_cleared_on_invalidate() {
        let mut cache = SessionCache::new();
        cache.store("/test.rs", "content");
        cache.set_compressed("/test.rs", "signatures", "cached sigs".to_string());
        cache.invalidate("/test.rs");
        assert_eq!(cache.get_compressed("/test.rs", "signatures"), None);
    }

    #[test]
    fn compressed_outputs_cleared_on_clear() {
        let mut cache = SessionCache::new();
        cache.store("/a.rs", "a");
        cache.set_compressed("/a.rs", "map", "map_a".to_string());
        cache.clear();
        assert_eq!(cache.get_compressed("/a.rs", "map"), None);
    }
}
