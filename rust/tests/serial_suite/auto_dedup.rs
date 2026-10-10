use sobre::core::cache::SessionCache;
use sobre::tools::autonomy::{AutonomyState, maybe_auto_dedup};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

fn init_test_data_dir() {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("tempdir"));
    // SAFETY: tests/serial.rs runs this binary on one thread, so no other
    // test touches the environment concurrently.
    unsafe { std::env::set_var("SOBRE_DATA_DIR", dir.path()) };
}

fn make_state() -> AutonomyState {
    init_test_data_dir();
    AutonomyState::new()
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
