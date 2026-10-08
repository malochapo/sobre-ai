use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::cache::SessionCache;

/// Tracks the one-shot cache deduplication.
pub struct AutonomyState {
    pub dedup_applied: AtomicBool,
    /// One-shot keys for large-output hints (`ctx_shell` bytes, `ctx_read` full tokens).
    pub large_output_hints_shown: Mutex<HashSet<String>>,
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
            large_output_hints_shown: Mutex::new(HashSet::new()),
        }
    }
}

/// Cache entries before deduplication runs.
const DEDUP_THRESHOLD: usize = 8;

/// Runs cache deduplication once the entry count exceeds the configured threshold.
pub fn maybe_auto_dedup(state: &AutonomyState, cache: &mut SessionCache) {
    if state
        .dedup_applied
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let entries = cache.get_all_entries();
    let threshold = DEDUP_THRESHOLD;
    if entries.len() < threshold {
        state.dedup_applied.store(false, Ordering::SeqCst);
        return;
    }

    let _ = crate::tools::impl_dedup::handle_action(cache, "apply");
}
