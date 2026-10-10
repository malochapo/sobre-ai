//! Path resolution on the read path returns instead of hanging: bounded
//! canonicalization falls back to the path as given, and reading a missing
//! file fails at once.
//!
//! The bounds below are generous on purpose: they catch a hang, not a slow
//! machine.

use sobre::core::pathutil::safe_canonicalize_bounded;
use std::path::Path;
use std::time::{Duration, Instant};

/// Far above any real canonicalization, far below a hang.
const NO_HANG: Duration = Duration::from_secs(5);

// ═══════════════════════════════════════════════════════════════════════════════
// safe_canonicalize_bounded
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn canonicalize_existing_path_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let result = safe_canonicalize_bounded(dir.path(), 2000);

    assert!(start.elapsed() < NO_HANG);
    assert!(
        result.is_absolute() && result.is_dir(),
        "an existing path resolves to an absolute path to it: {}",
        result.display()
    );
}

#[test]
fn canonicalize_nonexistent_path_returns_original() {
    let fake = Path::new("/nonexistent/path/that/does/not/exist/xyzzy123");
    let start = Instant::now();
    let result = safe_canonicalize_bounded(fake, 2000);

    assert!(start.elapsed() < NO_HANG);
    assert_eq!(result, fake.to_path_buf());
}

#[test]
fn canonicalize_bounded_returns_for_every_kind_of_path() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().to_string_lossy().into_owned();
    let paths = [
        existing.as_str(),
        "/this/does/not/exist",
        "relative/path/here",
    ];

    for p in paths {
        let start = Instant::now();
        let _ = safe_canonicalize_bounded(Path::new(p), 500);
        assert!(start.elapsed() < NO_HANG, "canonicalize hung for {p}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// read_file_lossy on paths that do not exist
// ═══════════════════════════════════════════════════════════════════════════════

mod read_file_safety {
    use super::NO_HANG;
    use sobre::tools::sobre_read::read_file_lossy;
    use std::time::Instant;

    #[test]
    fn nonexistent_file_errors_quickly() {
        let start = Instant::now();
        let result = read_file_lossy("/this/path/does/not/exist/file.rs");
        assert!(result.is_err());
        assert!(start.elapsed() < NO_HANG, "should fail fast, not hang");
    }

    #[test]
    fn deeply_nested_nonexistent_path_errors_quickly() {
        let deep = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/file.txt";
        let start = Instant::now();
        let result = read_file_lossy(deep);
        assert!(result.is_err());
        assert!(start.elapsed() < NO_HANG);
    }

    #[cfg(windows)]
    #[test]
    fn windows_profile_path_does_not_hang() {
        let path = r"C:\Users\USER\.gemini\some_config.json";
        let start = Instant::now();
        let _ = read_file_lossy(path);
        assert!(
            start.elapsed() < NO_HANG,
            "a path under the user profile must not hang"
        );
    }
}
