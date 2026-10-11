use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use super::bm25_index::BM25Index;

/// How long a loaded index is served before it is checked against the disk
/// again (a background refresh).
const TTL_SECS: u64 = 60;

/// Cheap content fingerprint of the persisted index file: `(mtime, size)`.
///
/// mtime alone is not enough — many filesystems only resolve mtime to 1–2 s, so
/// a background rebuild that lands in the same tick as the load would be missed.
/// Pairing it with the file size catches those same-second rewrites without the
/// cost of hashing a multi-MB index file on every per-query freshness check.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct IndexFingerprint {
    mtime: Option<SystemTime>,
    size: u64,
}

pub struct Bm25CacheEntry {
    pub root: PathBuf,
    pub index: Arc<BM25Index>,
    pub loaded_at: Instant,
    /// Fingerprint of the persisted index file when this entry was loaded.
    pub fingerprint: IndexFingerprint,
}

impl Bm25CacheEntry {
    pub fn is_fresh(&self) -> bool {
        if self.loaded_at.elapsed().as_secs() >= TTL_SECS {
            return false;
        }
        // Precise invalidation: if a background rebuild changed the index file
        // on disk, the resident copy is stale even within the TTL window.
        index_fingerprint(&self.root) == self.fingerprint
    }
}

/// `(mtime, size)` fingerprint of the persisted BM25 index file for `root`.
pub(crate) fn index_fingerprint(root: &Path) -> IndexFingerprint {
    match BM25Index::index_file_path(root).map(std::fs::metadata) {
        Some(Ok(m)) => IndexFingerprint {
            mtime: m.modified().ok(),
            size: m.len(),
        },
        _ => IndexFingerprint::default(),
    }
}

pub type SharedBm25Cache = std::sync::Arc<std::sync::Mutex<Option<Bm25CacheEntry>>>;

/// Get index from cache (fresh or stale), triggering background rebuild if stale.
/// Returns None only if no cache entry exists at all.
pub fn get_or_background(cache: &SharedBm25Cache, root: &Path) -> Option<Arc<BM25Index>> {
    let guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = guard.as_ref()?;
    if entry.root != root {
        return None;
    }

    let idx = Arc::clone(&entry.index);

    if !entry.is_fresh() {
        let root_str = root.to_string_lossy().to_string();
        let cache_clone = cache.clone();
        let root_clone = root.to_path_buf();
        std::thread::spawn(move || {
            // Isolate panics (corrupt index file, FS race): a panic here must not
            // kill the worker silently — the stale index keeps serving and the
            // next call retries the refresh.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rebuilt = BM25Index::load_or_build(&root_clone);
                let rebuilt_fp = index_fingerprint(&root_clone);
                let mut g = cache_clone
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *g = Some(Bm25CacheEntry {
                    root: root_clone,
                    index: Arc::new(rebuilt),
                    loaded_at: Instant::now(),
                    fingerprint: rebuilt_fp,
                });
            }));
            if result.is_ok() {
                tracing::debug!("[bm25_cache: background refresh done for {root_str}]");
            } else {
                tracing::warn!(
                    "[bm25_cache: background refresh panicked for {root_str}; serving stale index]"
                );
            }
        });
    }

    Some(idx)
}

/// Drops the cached BM25 index, freeing its heap memory.
/// The index will be rebuilt from disk on the next search.
pub fn unload(cache: &SharedBm25Cache) {
    let mut guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_some() {
        *guard = None;
        tracing::info!("[bm25_cache] unloaded index to free memory");
    }
}

/// Returns the approximate heap memory used by the cached BM25 index, or 0.
pub fn memory_usage(cache: &SharedBm25Cache) -> usize {
    let guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().map_or(0, |e| e.index.memory_usage_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn get_or_background_returns_none_on_empty() {
        let cache: SharedBm25Cache = Arc::new(std::sync::Mutex::new(None));
        let tmp = tempfile::tempdir().unwrap();
        assert!(get_or_background(&cache, tmp.path()).is_none());
    }

    #[test]
    fn fingerprint_default_when_index_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        // No persisted index file → default (None, 0) fingerprint.
        assert_eq!(index_fingerprint(tmp.path()), IndexFingerprint::default());
    }

    #[test]
    fn fingerprint_detects_size_change_under_equal_mtime() {
        // Two fingerprints with the same mtime but different size must differ,
        // proving size catches same-second rewrites that mtime alone misses.
        let mtime = Some(SystemTime::UNIX_EPOCH);
        let a = IndexFingerprint { mtime, size: 100 };
        let b = IndexFingerprint { mtime, size: 200 };
        assert_ne!(a, b);
        assert_eq!(a, IndexFingerprint { mtime, size: 100 });
    }
}
