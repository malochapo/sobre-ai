//! The MCP server's crash-loop guard: an agent host that restarts the server a
//! few times while starting up never waits on it, and `doctor --fix` clears
//! the same start log the server keeps.

use sobre::core::startup_guard::{
    CRASH_LOOP_MAX_BACKOFF_SECS, CRASH_LOOP_THRESHOLD, CRASH_LOOP_WINDOW_SECS, MCP_PROCESS_NAME,
    crash_loop_backoff, reset_crash_loop,
};
use std::time::{Duration, Instant};

/// Points `SOBRE_DATA_DIR` at `dir` until dropped. Callers hold
/// `test_env_lock()`.
struct DataDir;

impl DataDir {
    fn set(dir: &std::path::Path) -> Self {
        // SAFETY: serialized by `test_env_lock()`.
        unsafe { std::env::set_var("SOBRE_DATA_DIR", dir) };
        Self
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        // SAFETY: serialized by `test_env_lock()`.
        unsafe { std::env::remove_var("SOBRE_DATA_DIR") };
    }
}

#[test]
fn crash_loop_limits_tolerate_slow_hosts() {
    assert!(
        CRASH_LOOP_THRESHOLD >= 8,
        "a slow host may restart the server 5-7 times while starting up"
    );
    assert!(
        CRASH_LOOP_WINDOW_SECS >= 60,
        "a host can take 30-60 s to start"
    );
    assert!(
        CRASH_LOOP_MAX_BACKOFF_SECS <= 30,
        "a longer wait reads as a hung server"
    );
}

#[test]
fn restarts_under_the_threshold_do_not_wait() {
    let _env = sobre::core::data_dir::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let _data = DataDir::set(dir.path());

    let start = Instant::now();
    for _ in 0..7 {
        crash_loop_backoff("scenario-normal");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "7 starts within the window must not back off, took {elapsed:?}"
    );
}

#[test]
fn reset_removes_the_start_log() {
    let _env = sobre::core::data_dir::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let _data = DataDir::set(dir.path());

    for _ in 0..7 {
        crash_loop_backoff("scenario-reset");
    }
    reset_crash_loop("scenario-reset");
    assert!(
        !dir.path().join(".scenario-reset-starts.log").exists(),
        "reset must remove the start log"
    );

    let start = Instant::now();
    crash_loop_backoff("scenario-reset");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "after a reset, the next start must not back off"
    );
}

#[test]
fn reset_clears_the_log_the_server_writes() {
    let _env = sobre::core::data_dir::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let _data = DataDir::set(dir.path());

    for _ in 0..5 {
        crash_loop_backoff(MCP_PROCESS_NAME);
    }
    let log_path = dir.path().join(format!(".{MCP_PROCESS_NAME}-starts.log"));
    assert!(log_path.exists(), "the server records its starts");

    reset_crash_loop(MCP_PROCESS_NAME);
    assert!(
        !log_path.exists(),
        "reset_crash_loop with the same name must delete the log"
    );
}
