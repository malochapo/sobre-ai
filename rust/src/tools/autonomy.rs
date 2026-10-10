use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::cache::SessionCache;

/// Tracks the one-shot cache deduplication.
pub struct AutonomyState {
    pub dedup_applied: AtomicBool,
}

impl Default for AutonomyState {
    fn default() -> Self {
        Self::new()
    }
}

impl AutonomyState {
    /// Creates a new autonomy state.
    pub fn new() -> Self {
        Self {
            dedup_applied: AtomicBool::new(false),
        }
    }
}

/// Cache entries before deduplication runs.
const DEDUP_THRESHOLD: usize = 8;

/// Runs cache deduplication once, the first time the cache holds
/// `DEDUP_THRESHOLD` entries.
pub fn maybe_auto_dedup(state: &AutonomyState, cache: &mut SessionCache) {
    if state
        .dedup_applied
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    if cache.get_all_entries().len() < DEDUP_THRESHOLD {
        state.dedup_applied.store(false, Ordering::SeqCst);
        return;
    }

    crate::tools::impl_dedup::apply(cache);
}
