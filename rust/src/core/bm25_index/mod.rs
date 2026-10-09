use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
mod build;
mod chunking;
pub use chunking::*;
#[cfg(test)]
mod tests;

const MAX_BM25_FILES: usize = 5000;
const CHUNK_COUNT_WARNING: usize = 50_000;
const ZSTD_LEVEL: i32 = 9;

const DEFAULT_BM25_IGNORES: &[&str] = &[
    "vendor/**",
    "dist/**",
    "build/**",
    "public/vendor/**",
    "public/js/**",
    "public/css/**",
    "public/build/**",
    ".next/**",
    ".nuxt/**",
    "__pycache__/**",
    "*.min.js",
    "*.min.css",
    "*.bundle.js",
    "*.chunk.js",
];

fn max_bm25_cache_bytes() -> u64 {
    let mb = crate::core::config::DEFAULT_BM25_PERSIST_MB;
    mb * 1024 * 1024
}

/// Effective on-disk ceiling (bytes) for the persisted BM25 index. Single source
/// of truth shared with `doctor` so its "oversized index" warning matches what
/// `save`/`load` actually enforce.
pub fn persist_ceiling_bytes() -> u64 {
    max_bm25_cache_bytes()
}

/// Outcome of persisting a BM25 index to disk. Distinguishes a real write from a
/// size-capped refusal so callers never mistake "refused to persist" for
/// success (the bug behind the perpetual "index warming" report).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Written to disk. Carries the compressed (zstd) size in bytes.
    Persisted { compressed_bytes: u64 },
    /// Built fine but NOT written — the compressed size exceeds the disk
    /// ceiling. The in-memory index is still usable for this process; callers
    /// should surface the remedy (raise the cap / add ignore patterns) instead
    /// of silently rebuilding on every call.
    SkippedTooLarge {
        compressed_bytes: u64,
        limit_bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeChunk {
    pub file_path: String,
    pub symbol_name: String,
    pub kind: ChunkKind,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub tokens: Vec<String>,
    pub token_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChunkKind {
    Function,
    Struct,
    Impl,
    Module,
    Class,
    Method,
    Other,
    // -- External source kinds (Context Engine) --
    Issue,
    PullRequest,
    WikiPage,
    DbSchema,
    ApiEndpoint,
    Ticket,
    ExternalOther,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedFileState {
    pub mtime_ms: u64,
    pub size_bytes: u64,
}

impl IndexedFileState {
    fn from_path(path: &Path) -> Option<Self> {
        let meta = path.metadata().ok()?;
        let size_bytes = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)?;
        Some(Self {
            mtime_ms,
            size_bytes,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BM25Index {
    pub chunks: Vec<CodeChunk>,
    pub inverted: HashMap<String, Vec<(usize, f64)>>,
    pub avg_doc_len: f64,
    pub doc_count: usize,
    pub doc_freqs: HashMap<String, usize>,
    pub files: HashMap<String, IndexedFileState>,
    /// True once `shrink_resident_content_to_snippet` has trimmed each chunk's
    /// `content` down to the snippet lines. Resident-only RAM-saving state: never
    /// persisted (`skip`) so the on-disk index keeps full content, and a reload
    /// always starts as `false`. Guards the embedding pass against re-embedding
    /// truncated bodies (see `ensure_embeddings`).
    #[serde(default, skip)]
    pub content_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub chunk_idx: usize,
    pub score: f64,
    pub file_path: String,
    pub symbol_name: String,
    pub kind: ChunkKind,
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
}

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

fn structural_query_boost(query: &str, chunk: &CodeChunk) -> f64 {
    let raw_terms: std::collections::HashSet<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|term| term.len() >= 3)
        .map(str::to_ascii_lowercase)
        .collect();
    let path_terms: std::collections::HashSet<String> = tokenize(&chunk.file_path)
        .into_iter()
        .map(|term| term.to_ascii_lowercase())
        .collect();

    let mut boost = 0.0;
    if raw_terms.contains(&chunk.symbol_name.to_ascii_lowercase()) {
        boost += 32.0;
    }
    boost += raw_terms.intersection(&path_terms).count() as f64 * 8.0;

    let has_identifier = query.split_whitespace().any(|term| {
        let has_lower = term.chars().any(char::is_lowercase);
        let has_upper = term.chars().any(char::is_uppercase);
        (has_lower && has_upper) || term.contains('_') || term.contains('.')
    });
    if has_identifier {
        let extension = std::path::Path::new(&chunk.file_path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(extension.as_str(), "md" | "mdx" | "rst" | "txt" | "adoc") {
            boost -= 8.0;
        } else if !extension.is_empty() {
            boost += 4.0;
        }
    }
    boost
}

impl Default for BM25Index {
    fn default() -> Self {
        Self::new()
    }
}

impl BM25Index {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            inverted: HashMap::new(),
            avg_doc_len: 0.0,
            doc_count: 0,
            doc_freqs: HashMap::new(),
            files: HashMap::new(),
            content_truncated: false,
        }
    }

    /// Approximate heap memory used by this index in bytes.
    pub fn memory_usage_bytes(&self) -> usize {
        let chunks_size: usize = self
            .chunks
            .iter()
            .map(|c| {
                c.content.len()
                    + c.file_path.len()
                    + c.symbol_name.len()
                    + c.tokens.iter().map(String::len).sum::<usize>()
                    + 64
            })
            .sum();
        let inverted_size: usize = self
            .inverted
            .iter()
            .map(|(k, v)| k.len() + v.len() * 16 + 32)
            .sum();
        let files_size: usize = self.files.keys().map(|k| k.len() + 24).sum();
        let freqs_size: usize = self.doc_freqs.keys().map(|k| k.len() + 16).sum();
        chunks_size + inverted_size + files_size + freqs_size
    }

    /// Shrinks each resident chunk's `content` to its first `keep_lines` lines,
    /// reclaiming the RAM held by the full source bodies once the embedding pass
    /// has already consumed them. The search path only ever reads
    /// `content.lines().take(5)` for snippets, so the trimmed copy is functionally
    /// complete for BM25/dense/hybrid result rendering.
    ///
    /// Further shrinks resident content (chunks are already truncated to 10 lines
    /// during `add_chunk`; this method can tighten that further). Sets
    /// `content_truncated` so a later `ensure_embeddings` against this same
    /// resident index skips re-embedding (which would otherwise feed truncated
    /// bodies to the embedder).
    ///
    /// Idempotent and only ever shrinks: chunks shorter than `keep_lines` are
    /// left untouched.
    #[cfg(any(test, feature = "dense"))]
    pub fn shrink_resident_content_to_snippet(&mut self, keep_lines: usize) {
        let before = self.memory_usage_bytes();
        for chunk in &mut self.chunks {
            // Cheap line-count gate: only allocate a new string when the body is
            // actually longer than the snippet window.
            if chunk.content.lines().nth(keep_lines).is_some() {
                let trimmed: String = chunk
                    .content
                    .lines()
                    .take(keep_lines)
                    .collect::<Vec<_>>()
                    .join("\n");
                chunk.content = trimmed;
                // Reclaim the spare capacity left by the larger original body.
                chunk.content.shrink_to_fit();
            }
        }
        self.content_truncated = true;
        let after = self.memory_usage_bytes();
        tracing::debug!(
            "[bm25] shrank resident content to {keep_lines} lines/chunk, freed ~{:.1}MB",
            before.saturating_sub(after) as f64 / 1_048_576.0
        );
    }

    /// Builds an index from explicit chunks (unit tests; avoids filesystem walking).
    #[cfg(test)]
    pub(crate) fn from_chunks_for_test(chunks: Vec<CodeChunk>) -> Self {
        let mut index = Self::new();
        for mut chunk in chunks {
            if chunk.token_count == 0 {
                chunk.token_count = tokenize(&chunk.content).len();
            }
            index.add_chunk(chunk);
        }
        index.finalize();
        index
    }

    pub fn build_from_directory(root: &Path) -> Self {
        Self::build_from_directory_inner(root, &HashMap::new())
    }

    fn build_from_directory_inner(root: &Path, content_hint: &HashMap<String, String>) -> Self {
        let root_str = root.to_string_lossy();
        if !super::graph_index::is_safe_scan_root_public(&root_str) {
            tracing::warn!("[bm25: scan aborted for unsafe root {root_str}]");
            return Self::new();
        }
        let files = list_code_files(root);

        // Parallel fast path for the common case. The per-file parse +
        // tokenize work is pure and thread-safe, so we fan it across a rayon pool
        // and merge sequentially in file order — measured 2.85 s → 0.69 s (~4x) on
        // the sobre repo. Under memory pressure (or for tiny corpora, where pool
        // setup is not worth it) we fall back to the sequential build, which carries
        // the incremental early-break. Admission control: a corpus whose
        // estimated peak would blow past the guardian's hard threshold degrades to
        // the sequential build up front instead of OOMing mid-fan-out. Both paths
        // produce an identical index (see `build` module + determinism tests).
        if files.len() >= build::PARALLEL_MIN_FILES
            && !crate::core::memory_guard::is_under_pressure()
            && !crate::core::memory_guard::abort_requested()
            && crate::core::index_admission::admit_files(
                crate::core::index_admission::BuildKind::Bm25,
                root,
                &files,
            )
            .parallel_ok
        {
            return Self::build_parallel(root, content_hint, &files);
        }
        Self::build_sequential(root, content_hint, &files)
    }

    /// Group a previous index's chunks by file, each file's list sorted by
    /// (start_line, end_line, symbol_name) — the deterministic reuse order shared
    /// by both incremental rebuild paths (and the equivalence test, so it feeds
    /// them identical inputs).
    fn group_prev_chunks_by_file(prev: &BM25Index) -> HashMap<String, Vec<CodeChunk>> {
        let mut old_by_file: HashMap<String, Vec<CodeChunk>> = HashMap::new();
        for c in &prev.chunks {
            old_by_file
                .entry(c.file_path.clone())
                .or_default()
                .push(c.clone());
        }
        for v in old_by_file.values_mut() {
            v.sort_by(|a, b| {
                a.start_line
                    .cmp(&b.start_line)
                    .then_with(|| a.end_line.cmp(&b.end_line))
                    .then_with(|| a.symbol_name.cmp(&b.symbol_name))
            });
        }
        old_by_file
    }

    pub fn rebuild_incremental(root: &Path, prev: &BM25Index) -> Self {
        let old_by_file = Self::group_prev_chunks_by_file(prev);
        let files = list_code_files(root);

        // Mirror `build`'s dispatch. The edit loop is the hottest path in
        // daily use, and its serial cost is dominated by re-tokenizing the *many
        // unchanged* files' reused chunks — not the few changed ones. So the
        // parallel path fans the entire tokenization (changed files via
        // `prepare_file`, unchanged via re-`prepare_chunk`) across the rayon pool
        // and merges sequentially in file order. The sequential path keeps the
        // per-file memory-pressure early-break and serves small corpora / pressure.
        // Oversized corpora degrade to sequential via admission control.
        // Both produce an identical index (see determinism tests).
        if files.len() >= build::PARALLEL_MIN_FILES
            && !crate::core::memory_guard::is_under_pressure()
            && !crate::core::memory_guard::abort_requested()
            && crate::core::index_admission::admit_files(
                crate::core::index_admission::BuildKind::Bm25,
                root,
                &files,
            )
            .parallel_ok
        {
            return Self::rebuild_incremental_parallel(root, prev, &old_by_file, &files);
        }
        Self::rebuild_incremental_sequential(root, prev, &old_by_file, &files)
    }

    /// Sequential incremental rebuild with per-file memory-pressure guards. Reuses
    /// unchanged files' chunks and re-extracts changed ones. Used for small
    /// corpora and as the safe fallback under memory pressure.
    pub(crate) fn rebuild_incremental_sequential(
        root: &Path,
        prev: &BM25Index,
        old_by_file: &HashMap<String, Vec<CodeChunk>>,
        files: &[String],
    ) -> Self {
        let mut index = Self::new();
        const MAX_FILE_SIZE_BYTES: u64 = 2 * 1024 * 1024;
        let root_key = root.to_string_lossy().to_string();
        let total = files.len() as u64;
        crate::core::index_progress::report_bm25(&root_key, 0, total);

        for (i, rel) in files.iter().enumerate() {
            if i.is_multiple_of(16) || i + 1 == files.len() {
                crate::core::index_progress::report_bm25(&root_key, (i + 1) as u64, total);
            }
            if i.is_multiple_of(500) && crate::core::memory_guard::is_under_pressure() {
                tracing::warn!(
                    "[bm25: stopping incremental rebuild at file {i}/{} due to memory pressure]",
                    files.len()
                );
                break;
            }

            let abs = root.join(rel);
            let Some(state) = IndexedFileState::from_path(&abs) else {
                continue;
            };

            let unchanged = prev.files.get(rel).is_some_and(|old| *old == state);
            if unchanged
                && let Some(chunks) = old_by_file.get(rel)
                && chunks.first().is_some_and(|c| !c.content.is_empty())
            {
                for chunk in chunks {
                    index.add_chunk(chunk.clone());
                }
                index.files.insert(rel.clone(), state);
                continue;
            }

            if state.size_bytes > MAX_FILE_SIZE_BYTES {
                continue;
            }
            let content = if crate::core::extractors::is_binary_document(&abs) {
                match std::fs::read(&abs) {
                    Ok(bytes) => crate::core::extractors::text(&bytes),
                    Err(_) => continue,
                }
            } else {
                match std::fs::read_to_string(&abs) {
                    Ok(c) => c,
                    Err(_) => continue,
                }
            };
            if content.is_empty() {
                continue;
            }
            let mut chunks = extract_chunks(rel, &content);
            chunks.sort_by(|a, b| {
                a.start_line
                    .cmp(&b.start_line)
                    .then_with(|| a.end_line.cmp(&b.end_line))
                    .then_with(|| a.symbol_name.cmp(&b.symbol_name))
            });
            for chunk in chunks {
                index.add_chunk(chunk);
            }
            index.files.insert(rel.clone(), state);
        }

        index.finalize();
        index
    }

    fn add_chunk(&mut self, mut chunk: CodeChunk) {
        let idx = self.chunks.len();

        let enriched = enrich_for_bm25(&chunk);
        let tokens = tokenize(&enriched);
        for token in &tokens {
            let lower = token.to_lowercase();
            let postings = self.inverted.entry(lower.clone()).or_default();
            if postings.last().map(|(last_idx, _)| *last_idx) != Some(idx) {
                *self.doc_freqs.entry(lower).or_insert(0) += 1;
            }
            postings.push((idx, 1.0));
        }

        // Truncate content to snippet AFTER tokenization — full text was
        // used for BM25 scoring above; stored content is only for result display.
        const SNIPPET_LINES: usize = 10;
        if chunk.content.lines().nth(SNIPPET_LINES).is_some() {
            chunk.content = chunk
                .content
                .lines()
                .take(SNIPPET_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            chunk.content.shrink_to_fit();
        }

        self.chunks.push(CodeChunk {
            token_count: tokens.len(),
            tokens: Vec::new(),
            ..chunk
        });
    }

    fn finalize(&mut self) {
        self.doc_count = self.chunks.len();
        if self.doc_count == 0 {
            return;
        }

        let total_len: usize = self.chunks.iter().map(|c| c.token_count).sum();
        self.avg_doc_len = total_len as f64 / self.doc_count as f64;
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchResult> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || self.doc_count == 0 {
            return Vec::new();
        }

        // Pre-allocated score array: O(1) per-access vs HashMap overhead.
        // Kolmogorov-optimal: minimal allocation for the scoring operation.
        let n = self.chunks.len();
        let mut scores = vec![0.0f64; n];
        let mut touched = Vec::with_capacity(n.min(256));

        for token in &query_tokens {
            let lower = token.to_lowercase();
            let df = *self.doc_freqs.get(&lower).unwrap_or(&0) as f64;
            if df == 0.0 {
                continue;
            }

            let idf = ((self.doc_count as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();

            if let Some(postings) = self.inverted.get(&lower) {
                for &(idx, weight) in postings {
                    let doc_len = self.chunks[idx].token_count as f64;
                    let norm_len = doc_len / self.avg_doc_len.max(1.0);
                    let bm25 = idf * (weight * (BM25_K1 + 1.0))
                        / (weight + BM25_K1 * (1.0 - BM25_B + BM25_B * norm_len));

                    if scores[idx] == 0.0 {
                        touched.push(idx);
                    }
                    scores[idx] += bm25;
                }
            }
        }

        for &idx in &touched {
            scores[idx] += structural_query_boost(query, &self.chunks[idx]);
        }

        let mut results: Vec<SearchResult> = touched
            .iter()
            .filter(|&&idx| scores[idx] > 0.0)
            .map(|&idx| {
                let chunk = &self.chunks[idx];
                let snippet = chunk.content.lines().take(5).collect::<Vec<_>>().join("\n");
                SearchResult {
                    chunk_idx: idx,
                    score: scores[idx],
                    file_path: chunk.file_path.clone(),
                    symbol_name: chunk.symbol_name.clone(),
                    kind: chunk.kind.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    snippet,
                }
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.symbol_name.cmp(&b.symbol_name))
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.end_line.cmp(&b.end_line))
        });
        results.truncate(top_k);
        results
    }

    pub fn save(&self, root: &Path) -> std::io::Result<SaveOutcome> {
        if self.chunks.len() > CHUNK_COUNT_WARNING {
            tracing::warn!(
                "[bm25] index has {} chunks (threshold {}), consider adding paths to `ignore` in config.toml",
                self.chunks.len(),
                CHUNK_COUNT_WARNING
            );
        }

        let dir = index_dir(root).ok_or_else(|| std::io::Error::other("no data dir"))?;
        std::fs::create_dir_all(&dir)?;

        // Stream postcard → zstd → file to avoid holding the full
        // serialized buffer in memory. Temp-file + rename for atomicity.
        let target = dir.join("bm25_index.bin.zst");
        let tmp = dir.join("bm25_index.bin.zst.tmp");
        {
            let file = std::fs::File::create(&tmp)?;
            let buf_writer = std::io::BufWriter::new(file);
            let mut encoder = zstd::Encoder::new(buf_writer, ZSTD_LEVEL)
                .map_err(|e| std::io::Error::other(format!("zstd encoder init: {e}")))?;
            postcard::to_io(self, &mut encoder)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            encoder.finish()?;
        }

        let compressed_bytes = std::fs::metadata(&tmp)?.len();
        let max_bytes = max_bm25_cache_bytes();
        if compressed_bytes > max_bytes {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(
                "[bm25] compressed index too large ({:.1} MB, limit {:.0} MB), refusing to persist: {}",
                compressed_bytes as f64 / 1_048_576.0,
                max_bytes / (1024 * 1024),
                dir.display()
            );
            return Ok(SaveOutcome::SkippedTooLarge {
                compressed_bytes,
                limit_bytes: max_bytes,
            });
        }

        tracing::info!(
            "[bm25] index: {:.1} MB zstd compressed",
            compressed_bytes as f64 / 1_048_576.0,
        );

        std::fs::rename(&tmp, &target)?;

        let _ = std::fs::write(
            dir.join("project_root.txt"),
            root.to_string_lossy().as_bytes(),
        );

        Ok(SaveOutcome::Persisted { compressed_bytes })
    }

    pub fn load(root: &Path) -> Option<Self> {
        let dir = index_dir(root)?;
        let max_bytes = max_bm25_cache_bytes();

        let zst_path = dir.join("bm25_index.bin.zst");
        if zst_path.exists() {
            let meta = std::fs::metadata(&zst_path).ok()?;
            if meta.len() > max_bytes {
                tracing::warn!(
                    "[bm25] compressed index too large ({:.1} GB, limit {:.0} MB), quarantining: {}",
                    meta.len() as f64 / 1_073_741_824.0,
                    max_bytes / (1024 * 1024),
                    zst_path.display()
                );
                let quarantined = zst_path.with_extension("zst.quarantined");
                let _ = std::fs::rename(&zst_path, &quarantined);
                return None;
            }
            let compressed = std::fs::read(&zst_path).ok()?;
            let max_decompressed = max_bytes * 20; // allow 20x expansion ratio
            let data = bounded_zstd_decode(&compressed, max_decompressed)?;
            let idx: Self = postcard::from_bytes(&data).ok()?;
            return Some(idx);
        }

        None
    }

    pub fn load_or_build(root: &Path) -> Self {
        if !is_safe_bm25_root(root) {
            return Self::default();
        }
        if let Some(idx) = Self::load(root) {
            let stale = bm25_index_looks_stale(&idx, root);
            if !stale {
                return idx;
            }
            tracing::debug!(
                "[bm25_index: stale index detected for {}; rebuilding]",
                root.display()
            );
            let rebuilt = if idx.files.is_empty() {
                Self::build_from_directory(root)
            } else {
                Self::rebuild_incremental(root, &idx)
            };
            let _ = rebuilt.save(root);
            return rebuilt;
        }

        let built = Self::build_from_directory(root);
        let _ = built.save(root);
        built
    }

    pub fn index_file_path(root: &Path) -> Option<PathBuf> {
        Some(index_dir(root)?.join("bm25_index.bin.zst"))
    }

    /// Ingest external `ContentChunk`s into the BM25 index.
    /// Converts each chunk to a `CodeChunk` (backward-compatible) and
    /// rebuilds the inverted index. Returns the number of chunks ingested.
    pub fn ingest_content_chunks(
        &mut self,
        chunks: impl IntoIterator<Item = super::content_chunk::ContentChunk>,
    ) -> usize {
        let mut count = 0usize;
        for cc in chunks {
            self.add_chunk(cc.into());
            count += 1;
        }
        if count > 0 {
            self.finalize();
        }
        count
    }

    /// Remove every chunk whose `file_path` starts with `prefix` (e.g.
    /// `health://`) and rebuild the inverted index. Lets a recomputed source
    /// (like the code-health fabric) evict its prior pass so stale entries never
    /// linger in search. Returns the number of chunks removed.
    pub fn remove_chunks_with_prefix(&mut self, prefix: &str) -> usize {
        let before = self.chunks.len();
        self.chunks.retain(|c| !c.file_path.starts_with(prefix));
        let removed = before - self.chunks.len();
        if removed > 0 {
            self.finalize();
        }
        removed
    }
}

fn is_safe_bm25_root(root: &Path) -> bool {
    super::graph_index::is_safe_scan_root_public(&root.to_string_lossy())
}

fn bm25_index_looks_stale(index: &BM25Index, root: &Path) -> bool {
    if index.chunks.is_empty() {
        return false;
    }

    if index.files.is_empty() {
        let mut seen = std::collections::HashSet::<&str>::new();
        for chunk in &index.chunks {
            let rel = chunk.file_path.trim_start_matches(['/', '\\']);
            if rel.is_empty() {
                continue;
            }
            if !seen.insert(rel) {
                continue;
            }
            if !root.join(rel).exists() {
                return true;
            }
        }
        return false;
    }

    for (rel, old_state) in &index.files {
        let abs = root.join(rel);
        if !abs.exists() {
            return true;
        }
        let Some(cur) = IndexedFileState::from_path(&abs) else {
            return true;
        };
        if &cur != old_state {
            return true;
        }
    }

    for rel in list_code_files(root) {
        if !index.files.contains_key(&rel) {
            return true;
        }
    }

    false
}

fn bounded_zstd_decode(compressed: &[u8], max_bytes: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decoder = zstd::Decoder::new(compressed).ok()?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = decoder.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max_bytes {
            tracing::warn!(
                "[bm25] decompressed index exceeds limit ({:.0} MB > {:.0} MB), aborting load",
                total as f64 / (1024.0 * 1024.0),
                max_bytes as f64 / (1024.0 * 1024.0)
            );
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Some(buf)
}

fn index_dir(root: &Path) -> Option<PathBuf> {
    crate::core::index_namespace::vectors_dir(root)
}

fn list_code_files(root: &Path) -> Vec<String> {
    let cfg = crate::core::config::Config::load();
    // The declared corpus filter (config `ignore`) decides
    // membership before anything is chunked; the semantic index chunks this
    // corpus, so it inherits the same universe.
    let filter = crate::core::index_filter::IndexFileFilter::resolve(&cfg);

    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .max_depth(Some(20))
        .filter_entry(crate::core::walk_filter::keep_entry)
        .build();

    let mut ignore_patterns: Vec<glob::Pattern> = DEFAULT_BM25_IGNORES
        .iter()
        .filter_map(|p| glob::Pattern::new(p).ok())
        .collect();
    ignore_patterns.extend(cfg.ignore.iter().filter_map(|p| glob::Pattern::new(p).ok()));

    let mut files: Vec<String> = Vec::new();
    let mut filtered_out = 0usize;
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !crate::core::ingestion::is_ingestible(path) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        if rel.is_empty() {
            continue;
        }
        if ignore_patterns.iter().any(|p| p.matches(&rel)) {
            continue;
        }
        // Match on forward slashes so globs behave identically on Windows;
        // the index key keeps the platform separator (existing indexes stay
        // valid).
        if filter.is_excluded(&rel.replace('\\', "/")) {
            filtered_out += 1;
            continue;
        }
        if files.len() >= MAX_BM25_FILES {
            tracing::warn!(
                "[bm25] file cap reached ({MAX_BM25_FILES}), skipping remaining files in {}",
                root.display()
            );
            break;
        }
        files.push(rel);
    }

    if filtered_out > 0 {
        tracing::info!(
            "[bm25] index filter excluded {filtered_out} files ({})",
            filter.summary().unwrap_or_default()
        );
    }

    files.sort();
    files.dedup();
    files
}

pub fn is_code_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "rb"
            | "cs"
            | "kt"
            | "swift"
            | "php"
            | "scala"
            | "sql"
            | "ex"
            | "exs"
            | "zig"
            | "lua"
            | "dart"
            | "vue"
            | "svelte"
    )
}
