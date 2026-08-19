//! Detects tool-visibility config changes at runtime and notifies MCP clients.
//!
//! When the user changes `disabled_tools`
//! via the dashboard, CLI, or manual config edit, the MCP client (Cursor,
//! Claude Code, etc.) needs a `notifications/tools/list_changed` to re-fetch
//! `tools/list`. Without this, the client serves a stale tool surface until
//! the next IDE restart.
//!
//! The watcher computes a lightweight hash of the config fields that affect
//! tool visibility. Dispatch checks it on every tool call — a miss costs one
//! `Config::load()` (already cached by content-hash) plus a u64 comparison.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::config::Config;

/// Computes a stable hash of the config fields that determine the
/// `tools/list` response: `disabled_tools`. Two configs with the same hash
/// produce the same advertised tool set.
#[must_use]
pub fn tools_config_hash(cfg: &Config) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cfg.disabled_tools.hash(&mut hasher);
    hasher.finish()
}

/// Returns the current tools-config hash (loads config from disk).
#[must_use]
pub fn current_hash() -> u64 {
    tools_config_hash(&Config::load())
}

/// Checks whether the tools-relevant config has changed since the last
/// snapshot. If it has, atomically updates the stored hash and returns `true`.
/// The first call after initialization always returns `false` (the hash is
/// seeded in the constructor).
#[must_use]
pub fn has_changed(last_hash: &AtomicU64) -> bool {
    let now = current_hash();
    let prev = last_hash.load(Ordering::Relaxed);
    if now == prev {
        false
    } else {
        last_hash.store(now, Ordering::Relaxed);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_config_produces_same_hash() {
        let cfg = Config::default();
        assert_eq!(tools_config_hash(&cfg), tools_config_hash(&cfg));
    }

    #[test]
    fn different_disabled_tools_produces_different_hash() {
        let mut a = Config::default();
        let mut b = Config::default();
        a.disabled_tools = vec![];
        b.disabled_tools = vec!["ctx_call".to_string()];
        assert_ne!(tools_config_hash(&a), tools_config_hash(&b));
    }

    #[test]
    fn has_changed_returns_false_when_unchanged() {
        let hash = AtomicU64::new(current_hash());
        assert!(!has_changed(&hash));
    }

    #[test]
    fn has_changed_detects_difference() {
        let hash = AtomicU64::new(0);
        assert!(has_changed(&hash));
        assert!(!has_changed(&hash));
    }

    #[test]
    fn has_changed_stabilizes_after_update() {
        let hash = AtomicU64::new(0);
        assert!(has_changed(&hash), "first call with mismatched seed");
        let stored = hash.load(Ordering::Relaxed);
        assert_ne!(stored, 0, "hash should have been updated");
        assert!(!has_changed(&hash), "same config → no change");
        assert!(!has_changed(&hash), "still no change on third call");
    }
}
