use lean_ctx::core::cache::SessionCache;
use lean_ctx::tools::CrpMode;
use lean_ctx::tools::autonomy::{
    AutonomyState, is_session_overview_call, maybe_auto_dedup, session_lifecycle_pre_hook,
};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

fn init_test_data_dir() {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("tempdir"));
    // SAFETY: the project's test suite always runs with `--test-threads=1`
    // (env-race legacy — see .github/workflows/ci.yml), so no other test in
    // this binary touches the environment concurrently.
    unsafe { std::env::set_var("LEAN_CTX_DATA_DIR", dir.path()) };
}

fn make_state() -> AutonomyState {
    init_test_data_dir();
    AutonomyState::new()
}

/// A project with one source file and its graph built, as a graph tool would
/// leave it.
fn project_with_graph() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("project dir");
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    lean_ctx::core::graph_index::scan(&dir.path().to_string_lossy());
    dir
}

#[test]
fn session_lifecycle_fires_once() {
    let state = make_state();
    let mut cache = SessionCache::new();
    let project = project_with_graph();
    let root = project.path().to_string_lossy().to_string();

    let _first = session_lifecycle_pre_hook(
        &state,
        &mut cache,
        Some("fix auth bug"),
        Some(&root),
        CrpMode::Tdd,
    );

    let second = session_lifecycle_pre_hook(
        &state,
        &mut cache,
        Some("fix auth bug"),
        Some(&root),
        CrpMode::Tdd,
    );

    assert!(
        state.session_initialized.load(Ordering::SeqCst),
        "flag must be set after first call"
    );
    assert!(second.is_none(), "second call must return None");
}

#[test]
fn session_lifecycle_waits_for_a_graph_and_builds_none() {
    let state = make_state();
    let mut cache = SessionCache::new();
    let project = tempfile::tempdir().expect("project dir");
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"p\"\n",
    )
    .unwrap();
    let root = project.path().to_string_lossy().to_string();

    let result = session_lifecycle_pre_hook(
        &state,
        &mut cache,
        Some("fix auth bug"),
        Some(&root),
        CrpMode::Tdd,
    );

    assert!(result.is_none(), "no briefing without a graph");
    assert!(
        !state.session_initialized.load(Ordering::SeqCst),
        "the session stays eligible until a graph exists"
    );
    assert!(
        lean_ctx::core::graph_provider::open_best_effort(&root).is_none(),
        "the briefing must not build a graph"
    );
}

#[test]
fn session_lifecycle_skips_without_project_root() {
    let state = make_state();
    let mut cache = SessionCache::new();

    let result =
        session_lifecycle_pre_hook(&state, &mut cache, Some("fix auth bug"), None, CrpMode::Tdd);

    assert!(result.is_none(), "must skip when project_root is None");
    assert!(
        !state.session_initialized.load(Ordering::SeqCst),
        "flag must not be set without project_root"
    );
}

#[test]
fn session_overview_call_is_compose_without_task() {
    let no_args = serde_json::Map::new();
    let mut with_task = serde_json::Map::new();
    with_task.insert("task".into(), serde_json::json!("auth flow"));
    let mut empty_task = serde_json::Map::new();
    empty_task.insert("task".into(), serde_json::json!(""));

    assert!(is_session_overview_call("ctx_compose", None));
    assert!(is_session_overview_call("ctx_compose", Some(&no_args)));
    // ctx_compose routes any string task, even an empty one, to retrieval.
    assert!(!is_session_overview_call("ctx_compose", Some(&with_task)));
    assert!(!is_session_overview_call("ctx_compose", Some(&empty_task)));
    assert!(!is_session_overview_call("ctx_read", None));
}

#[test]
fn auto_dedup_fires_at_threshold() {
    let state = make_state();
    let mut cache = SessionCache::new();

    for i in 0..8 {
        let path = format!("test_file_{i}.rs");
        let content = format!("fn func_{i}() {{ println!(\"hello {i}\"); }}");
        cache.store(&path, &content);
    }

    maybe_auto_dedup(&state, &mut cache);
    assert!(
        state.dedup_applied.load(Ordering::SeqCst),
        "dedup must be applied at threshold"
    );
}

#[test]
fn auto_dedup_skips_below_threshold() {
    let state = make_state();
    let mut cache = SessionCache::new();

    for i in 0..3 {
        let path = format!("test_file_{i}.rs");
        cache.store(&path, &format!("content {i}"));
    }

    maybe_auto_dedup(&state, &mut cache);
    assert!(
        !state.dedup_applied.load(Ordering::SeqCst),
        "dedup must NOT be applied below threshold"
    );
}
