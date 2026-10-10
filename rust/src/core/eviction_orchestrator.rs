//! Turns the memory guard's RSS pressure level into read-cache and index
//! eviction.
//!
//! Each level maps to one action, mildest first:
//! - Soft: drop the cached `map`/`signatures` renders (the files stay cached).
//! - Medium: evict read-cache entries read only once.
//! - Hard: unload the search indices (BM25, content, ANN, resident indices).
//! - Critical: clear the read cache and every index.

use std::sync::{Arc, Mutex};

use super::bm25_cache::SharedBm25Cache;
use super::cache::SessionCache;
use super::memory_guard;

type SharedCache = Arc<tokio::sync::RwLock<SessionCache>>;

pub struct EvictionOrchestrator {
    cache: SharedCache,
    bm25_cache: SharedBm25Cache,
}

/// What the orchestrator does under memory pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvictionAction {
    /// Drop every cached `map`/`signatures` render; the files stay cached.
    TrimOutputs,
    /// Evict read-cache entries read only once.
    EvictProbationary,
    /// Unload the search indices from memory.
    UnloadIndices,
    /// Clear the read cache and every index.
    EmergencyDrop,
}

/// The level→action table. RSS pressure can come from structures the read
/// cache does not hold (ANN graph, resident trigram and graph indices, build
/// transients), so Hard and above always reach the indices.
fn action_for(level: memory_guard::PressureLevel) -> Option<EvictionAction> {
    match level {
        memory_guard::PressureLevel::Normal => None,
        memory_guard::PressureLevel::Soft => Some(EvictionAction::TrimOutputs),
        memory_guard::PressureLevel::Medium => Some(EvictionAction::EvictProbationary),
        memory_guard::PressureLevel::Hard => Some(EvictionAction::UnloadIndices),
        memory_guard::PressureLevel::Critical => Some(EvictionAction::EmergencyDrop),
    }
}

#[derive(Default)]
struct EvictionRegistry {
    targets: Vec<std::sync::Weak<EvictionOrchestrator>>,
}

impl EvictionRegistry {
    fn register(&mut self, target: &Arc<EvictionOrchestrator>) {
        self.targets.retain(|target| target.strong_count() > 0);
        self.targets.push(Arc::downgrade(target));
    }

    fn live_targets(&mut self) -> Vec<Arc<EvictionOrchestrator>> {
        let live = self
            .targets
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .collect();
        self.targets.retain(|target| target.strong_count() > 0);
        live
    }
}

static TARGETS: std::sync::LazyLock<Mutex<EvictionRegistry>> =
    std::sync::LazyLock::new(|| Mutex::new(EvictionRegistry::default()));

/// Register a server-local eviction target without extending its lifetime.
pub fn register(target: &Arc<EvictionOrchestrator>) {
    TARGETS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .register(target);
}

/// Fan process-wide RSS pressure out to every live server-local cache.
pub fn on_memory_pressure(level: memory_guard::PressureLevel) {
    let live = TARGETS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .live_targets();

    for target in live {
        target.on_pressure(level);
    }

    // These caches are process-global rather than owned by a server target.
    if level >= memory_guard::PressureLevel::Hard {
        super::content_cache::clear();
        #[cfg(feature = "dense")]
        super::ann_cache::clear();
        super::search_index::clear_resident();
    }
}

impl EvictionOrchestrator {
    pub fn new(cache: SharedCache, bm25_cache: SharedBm25Cache) -> Self {
        Self { cache, bm25_cache }
    }

    /// Called by the memory_guard thread when pressure is detected.
    /// Runs on the guardian thread — must not block on async locks for too long.
    pub fn on_pressure(&self, level: memory_guard::PressureLevel) {
        let Some(action) = action_for(level) else {
            return;
        };
        let bm25_bytes = super::bm25_cache::memory_usage(&self.bm25_cache);
        tracing::info!(
            "[eviction] pressure={level:?} bm25={:.1}MB action={action:?}",
            bm25_bytes as f64 / 1_048_576.0,
        );
        self.execute_action(action, bm25_bytes);
    }

    fn execute_action(&self, action: EvictionAction, bm25_bytes: usize) {
        match action {
            EvictionAction::TrimOutputs => {
                let trimmed = self.try_write_cache(SessionCache::trim_compressed_outputs);
                tracing::info!("[eviction] trimmed compressed outputs from {trimmed} entries");
            }

            EvictionAction::EvictProbationary => {
                let evicted = self.try_write_cache(|cache| {
                    let n = cache.evict_probationary();
                    cache.trim_shared_blocks();
                    n
                });
                tracing::info!("[eviction] evicted {evicted} probationary entries");
            }

            EvictionAction::UnloadIndices => {
                if bm25_bytes > 0 {
                    super::bm25_cache::unload(&self.bm25_cache);
                }
                let content_freed = super::content_cache::memory_usage_bytes();
                super::content_cache::clear();
                // The ANN/HNSW graph and the resident trigram indices
                // dominate RSS in embedding-heavy sessions, far beyond the
                // session cache.
                #[cfg(feature = "dense")]
                let ann_freed = {
                    let bytes = super::ann_cache::memory_usage_bytes();
                    super::ann_cache::clear();
                    bytes
                };
                #[cfg(not(feature = "dense"))]
                let ann_freed = 0usize;
                super::search_index::clear_resident();
                let trimmed = self.try_write_cache(SessionCache::trim_compressed_outputs);
                memory_guard::jemalloc_purge();
                tracing::info!(
                    "[eviction] unloaded indices (bm25={:.1}MB + content={:.1}MB + ann={:.1}MB freed, \
                     search residents dropped, {trimmed} outputs trimmed)",
                    bm25_bytes as f64 / 1_048_576.0,
                    content_freed as f64 / 1_048_576.0,
                    ann_freed as f64 / 1_048_576.0,
                );
            }

            EvictionAction::EmergencyDrop => {
                let cleared = self.try_write_cache(SessionCache::clear);
                super::bm25_cache::unload(&self.bm25_cache);
                super::content_cache::clear();
                // Emergency must reach every resident structure.
                #[cfg(feature = "dense")]
                super::ann_cache::clear();
                super::search_index::clear_resident();
                memory_guard::jemalloc_purge();
                tracing::warn!(
                    "[eviction] EMERGENCY: cleared {cleared} cache entries + unloaded all indices \
                     (bm25, content, ann, search)"
                );
            }
        }
    }

    /// Try to write to the cache. Returns default value if lock is contended.
    fn try_write_cache<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut SessionCache) -> R,
        R: Default,
    {
        if let Ok(mut guard) = self.cache.try_write() {
            f(&mut guard)
        } else {
            tracing::debug!("[eviction] cache write lock contended, skipping");
            R::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_orchestrator() -> EvictionOrchestrator {
        let cache = Arc::new(tokio::sync::RwLock::new(SessionCache::new()));
        let bm25_cache: SharedBm25Cache = Arc::new(std::sync::Mutex::new(None));
        EvictionOrchestrator::new(cache, bm25_cache)
    }

    #[test]
    fn registry_fans_out_and_prunes_dropped_targets() {
        // on_pressure(Critical) runs EmergencyDrop, which
        // clears the process-wide ann_cache — hold its cross-module test lock
        // so a concurrent ann_cache test can't observe a wiped cache mid-assertion.
        #[cfg(feature = "dense")]
        let _ann_lock = crate::core::ann_cache::test_lock();
        let first = Arc::new(make_orchestrator());
        let second = Arc::new(make_orchestrator());
        let first_cache = first.cache.clone();
        let second_cache = second.cache.clone();
        first_cache
            .blocking_write()
            .store("/first.rs", "fn first() {}");
        second_cache
            .blocking_write()
            .store("/second.rs", "fn second() {}");

        let mut registry = EvictionRegistry::default();
        registry.register(&first);
        registry.register(&second);
        let live = registry.live_targets();
        assert_eq!(live.len(), 2);
        for target in live {
            target.on_pressure(memory_guard::PressureLevel::Critical);
        }
        assert_eq!(first_cache.blocking_read().total_cached_tokens(), 0);
        assert_eq!(second_cache.blocking_read().total_cached_tokens(), 0);

        drop(second);
        assert_eq!(registry.live_targets().len(), 1);
        assert_eq!(registry.targets.len(), 1);
    }

    /// Soft pressure only trims renders: the BM25 index stays loaded.
    #[test]
    fn soft_pressure_keeps_indices_loaded() {
        let orch = make_orchestrator();
        *orch.bm25_cache.lock().unwrap() = Some(crate::core::bm25_cache::Bm25CacheEntry {
            root: std::path::PathBuf::new(),
            index: Arc::new(crate::core::bm25_index::BM25Index::new()),
            loaded_at: std::time::Instant::now(),
            fingerprint: crate::core::bm25_cache::IndexFingerprint::default(),
        });
        orch.on_pressure(memory_guard::PressureLevel::Soft);
        assert!(
            orch.bm25_cache.lock().unwrap().is_some(),
            "soft pressure must not unload the BM25 index"
        );
    }

    #[test]
    fn soft_pressure_with_empty_cache_is_noop() {
        let orch = make_orchestrator();
        orch.on_pressure(memory_guard::PressureLevel::Soft);
    }

    #[test]
    fn emergency_clears_cache() {
        // See `registry_fans_out_and_prunes_dropped_targets` above: EmergencyDrop
        // clears the shared ann_cache, so hold its cross-module test lock.
        #[cfg(feature = "dense")]
        let _ann_lock = crate::core::ann_cache::test_lock();
        let cache = Arc::new(tokio::sync::RwLock::new(SessionCache::new()));
        {
            let mut c = cache.blocking_write();
            c.store("/a.rs", "fn a() {}");
            c.store("/b.rs", "fn b() {}");
        }
        let bm25: SharedBm25Cache = Arc::new(std::sync::Mutex::new(None));
        let orch = EvictionOrchestrator::new(cache.clone(), bm25);

        orch.execute_action(EvictionAction::EmergencyDrop, 0);
        let c = cache.blocking_read();
        assert_eq!(c.total_cached_tokens(), 0);
    }

    #[test]
    fn trim_outputs_clears_compressed() {
        let cache = Arc::new(tokio::sync::RwLock::new(SessionCache::new()));
        {
            let mut c = cache.blocking_write();
            c.store("/a.rs", "fn main() {}");
            c.set_compressed("/a.rs", "map", "compressed map".to_string());
        }
        let bm25: SharedBm25Cache = Arc::new(std::sync::Mutex::new(None));
        let orch = EvictionOrchestrator::new(cache.clone(), bm25);

        orch.execute_action(EvictionAction::TrimOutputs, 0);
        let c = cache.blocking_read();
        assert!(c.get_compressed("/a.rs", "map").is_none());
    }

    #[test]
    fn hard_pressure_and_above_reach_the_indices() {
        use memory_guard::PressureLevel as L;
        assert_eq!(action_for(L::Normal), None);
        assert_eq!(action_for(L::Soft), Some(EvictionAction::TrimOutputs));
        assert_eq!(
            action_for(L::Medium),
            Some(EvictionAction::EvictProbationary)
        );
        assert_eq!(action_for(L::Hard), Some(EvictionAction::UnloadIndices));
        assert_eq!(action_for(L::Critical), Some(EvictionAction::EmergencyDrop));
    }

    #[test]
    fn evict_probationary_removes_single_reads() {
        let cache = Arc::new(tokio::sync::RwLock::new(SessionCache::new()));
        {
            let mut c = cache.blocking_write();
            c.store("/once.rs", "fn once() {}");
            c.store("/twice.rs", "fn twice() {}");
            c.store("/twice.rs", "fn twice() {}"); // second read → read_count=2
        }
        let bm25: SharedBm25Cache = Arc::new(std::sync::Mutex::new(None));
        let orch = EvictionOrchestrator::new(cache.clone(), bm25);

        orch.execute_action(EvictionAction::EvictProbationary, 0);
        let c = cache.blocking_read();
        assert!(c.get("/once.rs").is_none());
        assert!(c.get("/twice.rs").is_some());
    }
}
