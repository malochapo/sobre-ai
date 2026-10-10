//! Inline tests extracted from sobre_read.rs (LOC gate).
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn raw_alias_forces_raw_mode_over_explicit_mode() {
    // Raw=true is the verbatim escape hatch and must win over any
    // mode arg an agent also happened to pass.
    assert_eq!(
        resolve_raw_alias(true, Some("signatures".to_string())),
        Some("raw".to_string())
    );
    assert_eq!(resolve_raw_alias(true, None), Some("raw".to_string()));
}

#[test]
fn raw_alias_absent_passes_mode_through() {
    // Without raw=true the caller's mode is untouched (including None, which
    // lets the auto/policy/profile resolution downstream pick the mode).
    assert_eq!(
        resolve_raw_alias(false, Some("full".to_string())),
        Some("full".to_string())
    );
    assert_eq!(resolve_raw_alias(false, None), None);
}

#[test]
fn per_file_lock_same_path_returns_same_mutex() {
    let lock_a1 = per_file_lock("/tmp/test_same_path.txt");
    let lock_a2 = per_file_lock("/tmp/test_same_path.txt");
    assert!(Arc::ptr_eq(&lock_a1, &lock_a2));
}

#[test]
fn per_file_lock_different_paths_return_different_mutexes() {
    let lock_a = per_file_lock("/tmp/test_path_a.txt");
    let lock_b = per_file_lock("/tmp/test_path_b.txt");
    assert!(!Arc::ptr_eq(&lock_a, &lock_b));
}

#[test]
fn per_file_lock_serializes_concurrent_access() {
    let counter = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let path = "/tmp/test_concurrent_serialization.txt";
    let mut handles = Vec::new();

    for _ in 0..5 {
        let counter = counter.clone();
        let max_concurrent = max_concurrent.clone();
        let path = path.to_string();
        handles.push(std::thread::spawn(move || {
            let lock = per_file_lock(&path);
            let _guard = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let active = counter.fetch_add(1, Ordering::SeqCst) + 1;
            max_concurrent.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(10));
            counter.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(max_concurrent.load(Ordering::SeqCst), 1);
}

#[test]
fn per_file_lock_allows_parallel_different_paths() {
    let counter = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    for i in 0..4 {
        let counter = counter.clone();
        let max_concurrent = max_concurrent.clone();
        let path = format!("/tmp/test_parallel_{i}.txt");
        handles.push(std::thread::spawn(move || {
            let lock = per_file_lock(&path);
            let _guard = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let active = counter.fetch_add(1, Ordering::SeqCst) + 1;
            max_concurrent.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(50));
            counter.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert!(max_concurrent.load(Ordering::SeqCst) > 1);
}

// -- Regression: explicit mode and line 1 --
// Delegates to the real runtime helper so this test can never drift from
// production behaviour.
fn apply_start_line(
    mode: &mut String,
    fresh: &mut bool,
    explicit_mode: bool,
    start_line: Option<i64>,
) {
    super::apply_line_window(mode, fresh, explicit_mode, start_line, None, None);
}

#[test]
fn start_line_1_does_not_override_mode() {
    let mut mode = "auto".to_string();
    let mut fresh = false;
    apply_start_line(&mut mode, &mut fresh, false, Some(1));
    assert_eq!(mode, "auto", "start_line=1 should not change mode");
    assert!(!fresh, "start_line=1 should not force fresh=true");
}

#[test]
fn start_line_gt1_overrides_implicit_mode() {
    let mut mode = "auto".to_string();
    let mut fresh = false;
    apply_start_line(&mut mode, &mut fresh, false, Some(50));
    assert_eq!(mode, "lines:50-999999");
    assert!(fresh);
}

#[test]
fn start_line_gt1_overrides_explicit_map_to_lines() {
    // Start_line always wins — prevents full-file materialization
    // on large files. If only the map is needed, omit start_line.
    let mut mode = "map".to_string();
    let mut fresh = false;
    apply_start_line(&mut mode, &mut fresh, true, Some(50));
    assert_eq!(mode, "lines:50-999999");
    assert!(fresh);
}

#[test]
fn start_line_gt1_overrides_explicit_signatures_to_lines() {
    // Start_line always wins
    let mut mode = "signatures".to_string();
    let mut fresh = false;
    apply_start_line(&mut mode, &mut fresh, true, Some(100));
    assert_eq!(mode, "lines:100-999999");
    assert!(fresh);
}

/// Anchored + start_line + limit → anchored:N-M (preserves
/// anchor hashes for sobre_patch, streams only the window off disk).
#[test]
fn anchored_with_start_line_and_limit_becomes_windowed_anchored() {
    let mut mode = "anchored".to_string();
    let mut fresh = false;
    super::apply_line_window(&mut mode, &mut fresh, true, Some(715), None, Some(3));
    assert_eq!(mode, "anchored:715-717");
    assert!(fresh);
}

#[test]
fn start_line_gt1_honors_explicit_lines_mode() {
    let mut mode = "lines:1-50".to_string();
    let mut fresh = false;
    apply_start_line(&mut mode, &mut fresh, true, Some(30));
    assert_eq!(
        mode, "lines:30-999999",
        "explicit lines mode should accept start_line override"
    );
    assert!(fresh);
}

#[test]
fn start_line_none_does_nothing() {
    let mut mode = "map".to_string();
    let mut fresh = false;
    apply_start_line(&mut mode, &mut fresh, true, None);
    assert_eq!(mode, "map");
    assert!(!fresh);
}

#[test]
fn start_line_1_with_explicit_mode_preserves_it() {
    // OpenCode sends start_line=1 + mode=map — both should be preserved
    let mut mode = "map".to_string();
    let mut fresh = false;
    apply_start_line(&mut mode, &mut fresh, true, Some(1));
    assert_eq!(mode, "map");
    assert!(!fresh);
}

// -- Regression: `offset`/`limit` aliases --

#[test]
fn offset_is_alias_for_start_line() {
    let mut mode = "auto".to_string();
    let mut fresh = false;
    super::apply_line_window(&mut mode, &mut fresh, false, None, Some(40), None);
    assert_eq!(mode, "lines:40-999999");
    assert!(fresh);
}

#[test]
fn offset_and_limit_make_bounded_window() {
    let mut mode = "auto".to_string();
    let mut fresh = false;
    super::apply_line_window(&mut mode, &mut fresh, false, None, Some(40), Some(20));
    assert_eq!(mode, "lines:40-59", "20 inclusive lines starting at 40");
    assert!(fresh);
}

#[test]
fn limit_alone_reads_from_first_line() {
    let mut mode = "auto".to_string();
    let mut fresh = false;
    super::apply_line_window(&mut mode, &mut fresh, false, None, None, Some(25));
    assert_eq!(mode, "lines:1-25");
    assert!(fresh);
}

#[test]
fn start_line_wins_over_offset_when_both_present() {
    assert_eq!(
        super::resolve_line_window(Some(10), Some(99), None),
        Some((10, None))
    );
}

#[test]
fn resolve_clamps_start_and_drops_nonpositive_limit() {
    // Negative/zero start clamps to 1; non-positive limit is ignored.
    assert_eq!(
        super::resolve_line_window(Some(-5), None, Some(0)),
        Some((1, None))
    );
    // A bare non-positive limit yields no window at all.
    assert_eq!(super::resolve_line_window(None, None, Some(-3)), None);
    assert_eq!(super::resolve_line_window(None, None, None), None);
}

#[test]
fn lines_mode_bounds_are_inclusive() {
    assert_eq!(super::lines_mode(40, Some(20)), "lines:40-59");
    assert_eq!(super::lines_mode(5, None), "lines:5-999999");
}

#[test]
fn offset_limit_overrides_explicit_map_to_lines() {
    // Line window always wins to prevent full-file materialization
    let mut mode = "map".to_string();
    let mut fresh = false;
    super::apply_line_window(&mut mode, &mut fresh, true, None, Some(40), Some(20));
    assert_eq!(mode, "lines:40-59");
    assert!(fresh);
}

/// Schema/handler consistency: the handler reads
/// start_line/offset/limit, so the advertised schema must document them —
/// otherwise agents (and the generated tool reference) can't discover the
/// aliases and the divergence that caused this bug returns.
#[test]
fn schema_advertises_line_window_aliases() {
    let tool = SobreReadTool.tool_def();
    let props = tool
        .input_schema
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("sobre_read schema has a properties object");
    for key in ["path", "mode", "start_line", "offset", "limit", "fresh"] {
        assert!(props.contains_key(key), "sobre_read schema missing '{key}'");
    }
}
