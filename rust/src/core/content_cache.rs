//! Resident, bounded file-content cache shared across the search-index build and
//! `sobre_search`.
//!
//! The trigram [`search_index`](crate::core::search_index) build reads every
//! file of the corpus, and `sobre_search` then reads the narrowed candidate
//! files to run the regex line by line. This cache lets the first reader
//! (whichever it is) keep file contents, keyed by absolute path and validated
//! by `(mtime, size)`, so the next reader takes them from memory instead of
//! from disk.
//!
//! Correctness: an entry is only ever served when the file's *current*
//! `(mtime, size)` exactly matches the stored identity, and a file modified
//! less than `RACY_WINDOW_MS` (4 s) before it was read is not cached at all: a
//! same-size edit within one mtime tick (1 s on HFS+, 2 s on FAT) would
//! otherwise keep the old identity. A miss simply falls back to a disk read.
//!
//! Bounds & safety:
//! - Total resident bytes are capped (128 MB) with LRU eviction, so a large
//!   corpus cannot grow the cache without limit.
//! - Inserts are skipped while the process is under memory pressure, and the
//!   eviction orchestrator can [`clear`] the cache on `UnloadIndices` /
//!   `EmergencyDrop`.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use lru::LruCache;

/// Resident byte budget.
const BUDGET_BYTES: usize = 128 * 1024 * 1024;

/// A file whose mtime is this recent (or in the future) when it is inserted is
/// not cached: a later same-size edit could land in the same mtime tick. Twice
/// the coarsest common granularity (FAT's 2 s).
const RACY_WINDOW_MS: u64 = 4_000;

/// Identity of one file *version*. A changed mtime or size ⇒ stale ⇒ cache miss.
/// Mirrors the `(mtime, size)` pair the BM25 index already trusts for staleness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileState {
    pub mtime_ms: u64,
    pub size_bytes: u64,
}

impl FileState {
    /// Build from an already-`stat`ed [`Metadata`] (no extra syscall) — callers
    /// in the hot path typically have this in hand from their size/regular-file
    /// checks. Returns `None` only when the platform cannot report mtime.
    pub fn from_metadata(meta: &Metadata) -> Option<Self> {
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)?;
        Some(Self {
            mtime_ms,
            size_bytes: meta.len(),
        })
    }

    /// Whether this version is too recent to tell apart from a same-size edit
    /// in the same mtime tick.
    fn is_racy(self) -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        now_ms.saturating_sub(self.mtime_ms) < RACY_WINDOW_MS
    }
}

struct Entry {
    state: FileState,
    content: Arc<str>,
}

struct Cache {
    /// Unbounded by entry count — eviction is driven by `total_bytes` vs
    /// `budget_bytes` below; `LruCache` gives O(1) recency tracking and O(1)
    /// LRU-victim lookup (`pop_lru`).
    map: LruCache<PathBuf, Entry>,
    total_bytes: usize,
    budget_bytes: usize,
}

impl Cache {
    fn new(budget_bytes: usize) -> Self {
        Self {
            map: LruCache::unbounded(),
            total_bytes: 0,
            budget_bytes,
        }
    }

    fn remove_entry(&mut self, path: &Path) {
        if let Some(old) = self.map.pop(path) {
            self.total_bytes = self.total_bytes.saturating_sub(old.content.len());
        }
    }

    /// Evict LRU entries until the budget is satisfied. O(1) per eviction.
    fn evict_to_budget(&mut self) {
        while self.total_bytes > self.budget_bytes {
            let Some((_, victim)) = self.map.pop_lru() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(victim.content.len());
        }
    }
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::new(BUDGET_BYTES)))
}

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Look up `path`; returns the cached content only when the supplied current
/// `(mtime, size)` matches the stored identity. A mismatch evicts the stale
/// entry and reports a miss. `state` is passed in (not re-`stat`ed) because hot
/// callers already hold the metadata.
pub fn get(path: &Path, current: FileState) -> Option<Arc<str>> {
    let mut c = lock();
    // `peek` (not `get`) first: a stale hit must not promote to MRU on its way
    // to being evicted.
    let entry = c.map.peek(path)?;
    if entry.state != current {
        // Stale version cached — drop it so we don't keep paying for it.
        c.remove_entry(path);
        return None;
    }
    // `get` promotes to MRU.
    let entry = c.map.get(path)?;
    Some(Arc::clone(&entry.content))
}

/// Insert (or replace) the content for `path` at version `state`. Skipped while
/// the process is under memory pressure, so the cache never *adds* to a memory
/// problem, and for a file modified in the last `RACY_WINDOW_MS`.
pub fn insert(path: &Path, state: FileState, content: Arc<str>) {
    if state.is_racy() || crate::core::memory_guard::is_under_pressure() {
        return;
    }
    let len = content.len();
    let mut c = lock();
    // A single file larger than the whole budget would thrash eviction — skip it.
    if len > c.budget_bytes {
        return;
    }
    c.remove_entry(path);
    c.map.put(path.to_path_buf(), Entry { state, content });
    c.total_bytes += len;
    if c.total_bytes > c.budget_bytes {
        c.evict_to_budget();
    }
}

/// Drop all entries, freeing the heap. Called by the eviction orchestrator under
/// memory pressure; the cache simply re-warms on subsequent reads.
pub fn clear() {
    if CACHE.get().is_none() {
        return;
    }
    let mut c = lock();
    c.map.clear();
    c.total_bytes = 0;
}

/// Evict the least recently used `percent` of entries. Used after index builds
/// to release file contents that were only needed for chunking/tokenization.
/// `percent` is clamped to `[0, 100]`.
pub fn trim_oldest_percent(percent: u8) {
    if CACHE.get().is_none() {
        return;
    }
    let mut c = lock();
    let target_evictions = c.map.len() * usize::from(percent.min(100)) / 100;
    for _ in 0..target_evictions {
        let Some((_, victim)) = c.map.pop_lru() else {
            break;
        };
        c.total_bytes = c.total_bytes.saturating_sub(victim.content.len());
    }
}

/// Approximate resident heap used by cached contents, in bytes.
pub fn memory_usage_bytes() -> usize {
    if CACHE.get().is_none() {
        return 0;
    }
    lock().total_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cache is a process-wide global and tests mutate it. Serialize them
    /// so they cannot observe each other's state.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn fresh_cache(budget_bytes: usize) {
        let mut c = lock();
        *c = Cache::new(budget_bytes);
    }

    /// Writes a file last modified an hour ago, well outside the racy window.
    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
        p
    }

    #[test]
    fn hit_after_insert_with_matching_state() {
        let _g = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fresh_cache(1024 * 1024);
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "a.rs", "fn main() {}\n");
        let state = FileState::from_metadata(&std::fs::metadata(&p).unwrap()).unwrap();
        assert!(get(&p, state).is_none(), "cold cache must miss");
        insert(&p, state, Arc::from("fn main() {}\n"));
        let got = get(&p, state).expect("warm cache must hit");
        assert_eq!(&*got, "fn main() {}\n");
    }

    /// A file written in the last moments is not cached: a same-size edit in
    /// the same mtime tick would otherwise be served from the stale entry.
    #[test]
    fn recently_modified_file_is_not_cached() {
        let _g = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fresh_cache(1024 * 1024);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.rs");
        std::fs::write(&p, "let a = 1;\n").unwrap();
        let state = FileState::from_metadata(&std::fs::metadata(&p).unwrap()).unwrap();
        insert(&p, state, Arc::from("let a = 1;\n"));
        // Same size, same mtime tick: the cache cannot tell the versions apart.
        std::fs::write(&p, "let a = 2;\n").unwrap();
        assert!(
            get(&p, state).is_none(),
            "a racy entry must never be served"
        );
    }

    #[test]
    fn mtime_or_size_change_invalidates() {
        let _g = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fresh_cache(1024 * 1024);
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "a.rs", "v1\n");
        let s1 = FileState::from_metadata(&std::fs::metadata(&p).unwrap()).unwrap();
        insert(&p, s1, Arc::from("v1\n"));
        assert!(get(&p, s1).is_some());

        // Different size ⇒ different state ⇒ miss, and the stale entry is dropped.
        let s_bigger = FileState {
            size_bytes: s1.size_bytes + 10,
            ..s1
        };
        assert!(get(&p, s_bigger).is_none(), "size change must miss");
        assert!(
            get(&p, s1).is_none(),
            "stale entry must be evicted on mismatch"
        );

        // Different mtime ⇒ miss as well.
        insert(&p, s1, Arc::from("v1\n"));
        let s_newer = FileState {
            mtime_ms: s1.mtime_ms + 1,
            ..s1
        };
        assert!(get(&p, s_newer).is_none(), "mtime change must miss");
    }
}
