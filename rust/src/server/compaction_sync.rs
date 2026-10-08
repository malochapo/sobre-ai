use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::cache::SessionCache;

pub static LAST_COMPACTION_TS: AtomicU64 = AtomicU64::new(0);

/// Check if a host compaction event occurred since our last check.
/// If so, reset all `full_content_delivered` flags so the next read
/// delivers full content instead of a stub.
///
/// Detection (#808) reads the `last_compaction.json` marker file, written
/// atomically by the observe hook. It is only a few bytes, immune to being
/// displaced by large events.
pub fn sync_if_compacted(cache: &mut SessionCache, data_dir: &Path) -> bool {
    let last_seen = LAST_COMPACTION_TS.load(Ordering::Relaxed);

    let Some(latest_compaction_ts) = check_compaction_marker(data_dir, last_seen) else {
        return false;
    };

    LAST_COMPACTION_TS.store(latest_compaction_ts, Ordering::Relaxed);
    let reset_count = cache.reset_delivery_flags();
    // Drop the persistent stub index too (#955): the conversation's context was
    // summarised away, so neither a warm nor a cold stub may claim "you already
    // have this". Writes the emptied index synchronously so a restart in the
    // crash window can't resurrect a pre-compaction stub.
    crate::core::read_stub_index::reset_in_dir(data_dir);
    if reset_count > 0 {
        eprintln!(
            "[lean-ctx] compaction detected — reset {reset_count} delivery flags for re-read"
        );
    }

    std::thread::spawn(|| {
        if let Some(session) = crate::core::session::SessionState::load_latest()
            && let Some(ref root) = session.project_root
            && (!session.findings.is_empty() || !session.decisions.is_empty())
        {
            crate::tools::startup::auto_consolidate_knowledge(root);
        }
    });

    true
}

// ---------------------------------------------------------------------------
// Detection: marker file (#808)
// ---------------------------------------------------------------------------

/// Read `last_compaction.json` — a few-byte file written atomically by the
/// observe hook. Returns `Some(ts)` if the marker exists and its timestamp
/// is newer than `since_ts`.
fn check_compaction_marker(data_dir: &Path, since_ts: u64) -> Option<u64> {
    let marker_path = data_dir.join("last_compaction.json");
    let content = std::fs::read_to_string(&marker_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(content.trim()).ok()?;
    let ts = parsed.get("ts")?.as_u64()?;
    if ts > since_ts { Some(ts) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn make_cache_with_delivered(paths: &[&str]) -> SessionCache {
        let mut cache = SessionCache::default();
        for p in paths {
            cache.store(p, "hello world");
            cache.mark_full_delivered(p);
        }
        cache
    }

    fn write_marker(dir: &TempDir, ts: u64) {
        std::fs::write(
            dir.path().join("last_compaction.json"),
            format!(r#"{{"ts":{ts}}}"#),
        )
        .unwrap();
    }

    #[test]
    #[serial]
    fn no_reset_without_compaction_marker() {
        let dir = TempDir::new().unwrap();

        LAST_COMPACTION_TS.store(0, Ordering::Relaxed);
        let mut cache = make_cache_with_delivered(&["/tmp/a.rs"]);
        assert!(!sync_if_compacted(&mut cache, dir.path()));
        assert!(cache.is_full_delivered("/tmp/a.rs"));
    }

    #[test]
    #[serial]
    fn resets_after_compaction() {
        let dir = TempDir::new().unwrap();
        write_marker(&dir, 2000);

        LAST_COMPACTION_TS.store(0, Ordering::Relaxed);
        let mut cache = make_cache_with_delivered(&["/tmp/a.rs", "/tmp/b.rs"]);

        assert!(cache.is_full_delivered("/tmp/a.rs"));
        assert!(sync_if_compacted(&mut cache, dir.path()));
        assert!(!cache.is_full_delivered("/tmp/a.rs"));
        assert!(!cache.is_full_delivered("/tmp/b.rs"));
        assert_eq!(LAST_COMPACTION_TS.load(Ordering::Relaxed), 2000);
    }

    /// The same marker timestamp does not trigger a reset a second time.
    #[test]
    #[serial]
    fn does_not_double_reset() {
        let dir = TempDir::new().unwrap();
        write_marker(&dir, 2000);

        LAST_COMPACTION_TS.store(0, Ordering::Relaxed);
        let mut cache = make_cache_with_delivered(&["/tmp/a.rs"]);
        assert!(sync_if_compacted(&mut cache, dir.path()));
        assert!(!cache.is_full_delivered("/tmp/a.rs"));

        cache.mark_full_delivered("/tmp/a.rs");
        assert!(!sync_if_compacted(&mut cache, dir.path()));
        assert!(cache.is_full_delivered("/tmp/a.rs"));
    }

    /// A corrupt marker file is ignored.
    #[test]
    #[serial]
    fn corrupt_marker_is_ignored() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("last_compaction.json"), "not valid json").unwrap();

        LAST_COMPACTION_TS.store(0, Ordering::Relaxed);
        let mut cache = make_cache_with_delivered(&["/tmp/corrupt.rs"]);
        assert!(!sync_if_compacted(&mut cache, dir.path()));
        assert!(cache.is_full_delivered("/tmp/corrupt.rs"));
    }
}
