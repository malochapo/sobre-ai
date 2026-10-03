// ===== Group A: Index Scoping & Dashboard =====

#[test]
fn load_or_build_does_not_use_legacy_dot_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("myproject");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("main.rs"), "fn main() {}").unwrap();

    let dot_dir = tmp.path().join("graphs").join("dot-legacy");
    std::fs::create_dir_all(&dot_dir).unwrap();

    let idx = lean_ctx::core::graph_index::load_or_build(project.to_str().unwrap());
    assert!(
        idx.project_root.contains("myproject") || idx.files.is_empty(),
        "Should not load from a legacy '.' cache"
    );
}

// ===== Group B: Index Status =====

#[test]
fn disk_status_shows_exists_false_for_nonexistent_project() {
    let ds = lean_ctx::core::index_orchestrator::disk_status("/nonexistent/project/xyz123");
    assert!(!ds.graph_index.exists);
    assert!(!ds.bm25_index.exists);
    assert!(!ds.code_graph.exists);
}

#[test]
fn status_json_includes_disk_section() {
    let json = lean_ctx::core::index_orchestrator::status_json("/tmp/test-status-json");
    let val: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(val.get("disk").is_some(), "status_json must include 'disk'");
    let disk = val.get("disk").unwrap();
    assert!(disk.get("graph_index").is_some());
    assert!(disk.get("bm25_index").is_some());
    assert!(disk.get("code_graph").is_some());
}

#[test]
fn disk_status_graph_index_not_built() {
    let ds = lean_ctx::core::index_orchestrator::disk_status("/tmp/no-such-project-12345");
    assert!(!ds.graph_index.exists);
    assert!(ds.graph_index.size_bytes.is_none());
    assert!(ds.graph_index.modified_at.is_none());
}

// ===== Group C: Workflow Scoping =====
// These tests use serial execution via unique LEAN_CTX_DATA_DIR per test to avoid env var races.

#[test]
fn prune_caches_handles_empty_isolated_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::new("LEAN_CTX_DATA_DIR", tmp.path().to_str().unwrap());

    let result = lean_ctx::cli::prune_graph_caches();
    assert_eq!(result.removed, 0);
    let result2 = lean_ctx::cli::prune_bm25_caches();
    assert_eq!(result2.removed, 0);
}

/// Guards env var modifications so parallel tests don't race.
/// Uses a process-wide mutex to serialize all env-mutating tests.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn new(key: &'static str, val: &str) -> Self {
        use std::sync::{Mutex, OnceLock};
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let prev = std::env::var(key).ok();
        // SAFETY: serialized by this file's local `ENV_LOCK` mutex.
        unsafe { std::env::set_var(key, val) };
        Self {
            key,
            prev,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(ref v) = self.prev {
            // SAFETY: serialized by this file's local `ENV_LOCK` mutex.
            unsafe { std::env::set_var(self.key, v) };
        } else {
            // SAFETY: serialized by this file's local `ENV_LOCK` mutex.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}
