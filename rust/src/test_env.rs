//! Test-only helpers for mutating the process environment, plus
//! [`write_newer`] for tests that need a file to read as modified.
//!
//! `std::env::set_var` / `std::env::remove_var` became `unsafe` in Rust 2024
//! because they are not thread-safe: a concurrent environment read from another
//! thread is undefined behaviour. lean-ctx's tests serialize every environment
//! mutation through [`crate::core::data_dir::test_env_lock`], so the precondition
//! holds and the call is sound. Centralising the `unsafe` here documents that
//! invariant exactly once instead of at hundreds of call sites, while keeping
//! `#![warn(clippy::undocumented_unsafe_blocks)]` strict everywhere else.

use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Sets `key` to `value` in the process environment (test-only).
pub(crate) fn set_var<K: AsRef<OsStr>, V: AsRef<OsStr>>(key: K, value: V) {
    // SAFETY: tests serialize all environment access through test_env_lock(),
    // so no other thread reads or writes the environment concurrently.
    unsafe { std::env::set_var(key, value) };
}

/// Removes `key` from the process environment (test-only).
pub(crate) fn remove_var<K: AsRef<OsStr>>(key: K) {
    // SAFETY: tests serialize all environment access through test_env_lock(),
    // so no other thread reads or writes the environment concurrently.
    unsafe { std::env::remove_var(key) };
}

/// Writes `contents` to `path`, then sets its mtime at least 2 s past both the
/// previous mtime and now (test-only). Staleness checks then see a newer file
/// without the test sleeping out the filesystem's timestamp granularity.
pub(crate) fn write_newer(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    let before = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    std::fs::write(path, contents).unwrap();
    let now = SystemTime::now();
    let floor = before.map_or(now, |b| b.max(now));
    filetime::set_file_mtime(
        path,
        filetime::FileTime::from_system_time(floor + Duration::from_secs(2)),
    )
    .unwrap();
}
