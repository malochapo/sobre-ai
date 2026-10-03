//! Shared progress counters for index builds (BM25, semantic, graph).
//! Build code reports here; CLI / status_json read snapshots. Decouples
//! builders from the orchestrator (no module cycles).
//!
//! Prefer [`ProgressGuard`] at phase boundaries so counters clear on every exit
//! path (including panics via `Drop`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexComponent {
    Graph,
    Bm25,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgressSnapshot {
    pub done: u64,
    pub total: u64,
}

impl ProgressSnapshot {
    /// `true` when a known total exists (determinate bar).
    #[cfg(test)]
    pub fn is_determinate(&self) -> bool {
        self.total > 0
    }

    pub fn percent(&self) -> Option<u8> {
        if self.total == 0 {
            return None;
        }
        let pct = (self.done.saturating_mul(100)) / self.total;
        Some(pct.min(100) as u8)
    }
}

/// Clears the component counter when dropped (including panic unwind).
pub struct ProgressGuard {
    root: String,
    component: IndexComponent,
    cleared: bool,
}

impl ProgressGuard {
    pub fn new(root: impl Into<String>, component: IndexComponent) -> Self {
        Self {
            root: root.into(),
            component,
            cleared: false,
        }
    }

    pub fn report(&self, done: u64, total: u64) {
        report(&self.root, self.component, done, total);
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if !self.cleared {
            clear(&self.root, self.component);
        }
    }
}

type ProgressMap = HashMap<(String, IndexComponent), ProgressSnapshot>;

fn map() -> &'static Mutex<ProgressMap> {
    static MAP: OnceLock<Mutex<ProgressMap>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Report progress for a project root + component.
/// `total == 0` means indeterminate (spinner / bouncing arrow).
pub fn report(root: &str, component: IndexComponent, done: u64, total: u64) {
    let mut g = map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    g.insert(
        (root.to_string(), component),
        ProgressSnapshot { done, total },
    );
}

pub fn get(root: &str, component: IndexComponent) -> ProgressSnapshot {
    let g = map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    g.get(&(root.to_string(), component))
        .copied()
        .unwrap_or_default()
}

pub fn clear(root: &str, component: IndexComponent) {
    let mut g = map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    g.remove(&(root.to_string(), component));
}

#[cfg(test)]
pub fn clear_root(root: &str) {
    let mut g = map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    g.retain(|(r, _), _| r != root);
}

/// Convenience for BM25 file-count progress (avoids repeating the component).
pub fn report_bm25(root: &str, done: u64, total: u64) {
    report(root, IndexComponent::Bm25, done, total);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_get_clear() {
        let root = "__test_index_progress_report__";
        clear_root(root);
        report(root, IndexComponent::Bm25, 3, 10);
        let s = get(root, IndexComponent::Bm25);
        assert_eq!(s.done, 3);
        assert_eq!(s.total, 10);
        assert_eq!(s.percent(), Some(30));
        assert!(s.is_determinate());
        clear(root, IndexComponent::Bm25);
        assert_eq!(get(root, IndexComponent::Bm25), ProgressSnapshot::default());
    }

    #[test]
    fn indeterminate_when_total_zero() {
        let root = "__test_index_progress_indet__";
        clear_root(root);
        report(root, IndexComponent::Graph, 0, 0);
        let s = get(root, IndexComponent::Graph);
        assert!(!s.is_determinate());
        assert_eq!(s.percent(), None);
        clear_root(root);
    }

    #[test]
    fn percent_caps_at_100() {
        let root = "__test_index_progress_pct__";
        clear_root(root);
        report(root, IndexComponent::Semantic, 15, 10);
        assert_eq!(get(root, IndexComponent::Semantic).percent(), Some(100));
        clear_root(root);
    }

    #[test]
    fn guard_clears_on_drop() {
        let root = "__test_index_progress_guard__";
        clear_root(root);
        {
            let g = ProgressGuard::new(root, IndexComponent::Semantic);
            g.report(1, 4);
            assert_eq!(get(root, IndexComponent::Semantic).done, 1);
        }
        assert_eq!(
            get(root, IndexComponent::Semantic),
            ProgressSnapshot::default()
        );
        clear_root(root);
    }

    #[test]
    fn guard_clears_after_panic() {
        let root = "__test_index_progress_guard_panic__";
        clear_root(root);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let g = ProgressGuard::new(root, IndexComponent::Bm25);
            g.report(2, 5);
            panic!("boom");
        }));
        assert_eq!(get(root, IndexComponent::Bm25), ProgressSnapshot::default());
        clear_root(root);
    }
}
