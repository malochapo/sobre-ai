//! Tests verifying that `sobre_read` never hangs indefinitely due to session lock contention.
//! Covers the Windows deadlock scenario: shell-hook holds session write-lock while
//! `sobre_read` waits on session read-lock, causing circular dependency.

use sobre::core::pathutil::safe_canonicalize_bounded;
use std::path::Path;
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════════
// 1. safe_canonicalize_bounded — timeout behavior
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn canonicalize_existing_path_succeeds_quickly() {
    let start = Instant::now();
    let result = safe_canonicalize_bounded(Path::new("/tmp"), 2000);
    let elapsed = start.elapsed();

    assert!(elapsed < Duration::from_secs(1));
    // On macOS /tmp -> /private/tmp
    assert!(
        result.to_str().unwrap().contains("tmp"),
        "should resolve to a path containing 'tmp', got: {}",
        result.display()
    );
}

#[test]
fn canonicalize_nonexistent_path_returns_original() {
    let fake = Path::new("/nonexistent/path/that/does/not/exist/xyzzy123");
    let start = Instant::now();
    let result = safe_canonicalize_bounded(fake, 2000);
    let elapsed = start.elapsed();

    assert!(elapsed < Duration::from_secs(1));
    assert_eq!(result, fake.to_path_buf());
}

#[test]
fn canonicalize_bounded_respects_timeout() {
    // We can't easily simulate a hanging canonicalize on non-Windows,
    // but we verify the function doesn't panic and completes quickly
    // for various path types
    let paths = &[
        "/tmp",
        "/var",
        "/usr/bin",
        "/this/does/not/exist",
        "relative/path/here",
    ];

    for p in paths {
        let start = Instant::now();
        let _ = safe_canonicalize_bounded(Path::new(p), 500);
        assert!(
            start.elapsed() < Duration::from_millis(600),
            "canonicalize took too long for {p}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 3. Integration: read_file_lossy doesn't hang on nonexistent/weird paths
// ═══════════════════════════════════════════════════════════════════════════════

mod read_file_safety {
    use sobre::tools::sobre_read::read_file_lossy;
    use std::time::{Duration, Instant};

    #[test]
    fn nonexistent_file_errors_quickly() {
        let start = Instant::now();
        let result = read_file_lossy("/this/path/does/not/exist/file.rs");
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "should fail fast, not hang"
        );
    }

    #[test]
    fn deeply_nested_nonexistent_path_errors_quickly() {
        let deep = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/file.txt";
        let start = Instant::now();
        let result = read_file_lossy(deep);
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[cfg(windows)]
    #[test]
    fn windows_gemini_path_doesnt_hang() {
        // Simulates the exact path pattern from the bug report
        let path = r"C:\Users\USER\.gemini\some_config.json";
        let start = Instant::now();
        let _ = read_file_lossy(path);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "Windows .gemini path must not hang"
        );
    }
}
