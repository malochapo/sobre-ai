use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

pub const CRASH_LOOP_WINDOW_SECS: u64 = 60;
pub const CRASH_LOOP_THRESHOLD: usize = 8;
pub const CRASH_LOOP_MAX_BACKOFF_SECS: u64 = 30;

pub const MCP_PROCESS_NAME: &str = "mcp-server";

/// The file of recent start times `crash_loop_backoff` keeps for
/// `process_name`.
pub fn crash_loop_log_path(process_name: &str) -> Option<PathBuf> {
    crate::core::data_dir::sobre_data_dir()
        .ok()
        .map(|dir| dir.join(format!(".{}-starts.log", sanitize_lock_name(process_name))))
}

/// A held cross-process lock: an OS advisory lock (`flock` on Unix,
/// `LockFileEx` on Windows) on `.<name>.lock` in the data directory. The OS
/// releases it when the guard drops or when its process dies, so no lock ever
/// outlives its holder. The lock file itself stays, empty: deleting it while
/// another process waits on it would let two holders lock two different files.
pub struct StartupLockGuard {
    _file: std::fs::File,
}

fn sanitize_lock_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Takes the cross-process lock `name`, waiting up to `wait` for its holder to
/// release it.
///
/// Returns `None` if the data dir can't be resolved, the lock file can't be
/// opened, or the lock is still held after `wait`.
pub fn try_acquire_lock(name: &str, wait: Duration) -> Option<StartupLockGuard> {
    let dir = crate::core::data_dir::sobre_data_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!(".{}.lock", sanitize_lock_name(name)));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .ok()?;

    let deadline = std::time::Instant::now().checked_add(wait)?;
    let mut sleep_ms: u64 = 10;
    // std has no timed lock: poll `try_lock` until the deadline.
    loop {
        match file.try_lock() {
            Ok(()) => return Some(StartupLockGuard { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {}
            // A filesystem without advisory locks: run unguarded rather than
            // never building.
            Err(std::fs::TryLockError::Error(e)) => {
                tracing::debug!("[startup_guard: cannot lock {}: {e}]", path.display());
                return Some(StartupLockGuard { _file: file });
            }
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(sleep_ms).min(remaining));
        sleep_ms = (sleep_ms.saturating_mul(2)).min(120);
    }
}

/// Detects rapid restart loops (e.g., IDE keeps respawning a crashing MCP server).
/// Records each startup timestamp; if too many happen within the window, sleeps
/// with exponential backoff to break the loop and avoid host degradation.
pub fn crash_loop_backoff(process_name: &str) {
    let Some(ts_path) = crash_loop_log_path(process_name) else {
        return;
    };
    if let Some(dir) = ts_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let cutoff = now.saturating_sub(CRASH_LOOP_WINDOW_SECS);

    let mut recent: Vec<u64> = std::fs::read_to_string(&ts_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .filter(|&ts| ts >= cutoff)
        .collect();
    recent.push(now);

    if let Ok(mut f) = std::fs::File::create(&ts_path) {
        for ts in &recent {
            let _ = writeln!(f, "{ts}");
        }
    }

    if recent.len() > CRASH_LOOP_THRESHOLD {
        let restarts_over = recent.len() - CRASH_LOOP_THRESHOLD;
        let backoff_secs =
            (2u64.saturating_pow(restarts_over as u32)).min(CRASH_LOOP_MAX_BACKOFF_SECS);
        let msg = format!(
            "sobre: crash-loop protection — {process_name} started {} times in {CRASH_LOOP_WINDOW_SECS}s, \
             waiting {backoff_secs}s before accepting connections. \
             If your IDE is slow to initialize, this is normal.",
            recent.len()
        );
        tracing::warn!("{msg}");
        eprintln!("{msg}");
        std::thread::sleep(Duration::from_secs(backoff_secs));
    }
}

/// Clears the crash-loop history file, resetting any active backoff.
pub fn reset_crash_loop(process_name: &str) {
    if let Some(ts_path) = crash_loop_log_path(process_name) {
        let _ = std::fs::remove_file(ts_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let prev = std::env::var(key).ok();
            crate::test_env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.prev.as_deref() {
                Some(v) => crate::test_env::set_var(self.key, v),
                None => crate::test_env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn lock_acquire_and_release() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        let g = try_acquire_lock("unit-test", Duration::from_millis(200));
        assert!(g.is_some());
        assert!(dir.path().join(".unit-test.lock").exists());

        drop(g);
        assert!(
            try_acquire_lock("unit-test", Duration::ZERO).is_some(),
            "a dropped guard releases the lock"
        );
    }

    #[test]
    fn lock_times_out_while_held() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        let g1 = try_acquire_lock("unit-test-2", Duration::from_millis(200))
            .expect("first lock should acquire");
        let g2 = try_acquire_lock("unit-test-2", Duration::from_millis(60));
        assert!(g2.is_none());

        drop(g1);
        let g3 = try_acquire_lock("unit-test-2", Duration::from_millis(200));
        assert!(g3.is_some());
    }

    #[test]
    fn waiter_gets_the_lock_once_the_holder_releases_it() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        let held = try_acquire_lock("unit-test-3", Duration::ZERO).expect("first lock");
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(held);
        });
        assert!(
            try_acquire_lock("unit-test-3", Duration::from_secs(10)).is_some(),
            "a waiter must take the lock once its holder releases it"
        );
        releaser.join().unwrap();
    }

    #[test]
    fn leftover_lock_file_does_not_block() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        // A lock file whose holder has gone: the file is there, the OS lock
        // is not.
        std::fs::write(dir.path().join(".leftover.lock"), "").unwrap();
        assert!(try_acquire_lock("leftover", Duration::ZERO).is_some());
    }

    #[test]
    fn crash_loop_thresholds_are_resilient() {
        let threshold = CRASH_LOOP_THRESHOLD;
        let window = CRASH_LOOP_WINDOW_SECS;
        let backoff = CRASH_LOOP_MAX_BACKOFF_SECS;
        assert!(
            threshold >= 8,
            "threshold must tolerate IDE restart patterns (was {threshold})"
        );
        assert!(
            window >= 60,
            "window must cover slow IDE startup (was {window}s)"
        );
        assert!(
            backoff <= 30,
            "max backoff must not be too aggressive (was {backoff}s)"
        );
    }

    #[test]
    fn crash_loop_backoff_under_threshold_no_sleep() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        let start = std::time::Instant::now();
        for _ in 0..CRASH_LOOP_THRESHOLD {
            crash_loop_backoff("test-no-sleep");
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "under threshold should not sleep (elapsed {:?})",
            start.elapsed()
        );
    }

    #[test]
    fn reset_crash_loop_clears_history() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        for _ in 0..5 {
            crash_loop_backoff("test-reset");
        }
        let log_path = dir.path().join(".test-reset-starts.log");
        assert!(log_path.exists(), "crash loop log should exist after calls");

        reset_crash_loop("test-reset");
        assert!(
            !log_path.exists(),
            "crash loop log should be removed after reset"
        );
    }

    #[test]
    fn reset_crash_loop_nonexistent_is_noop() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        reset_crash_loop("never-existed");
    }

    /// Multi-window scenario: every healthy server resets the start
    /// history after its completed handshake, so N windows × M client retries
    /// can never accumulate into a fake crash loop whose pre-handshake backoff
    /// would then *cause* the client timeouts it exists to prevent.
    #[test]
    fn handshake_reset_keeps_healthy_restarts_below_threshold() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        let start = std::time::Instant::now();
        // Two full rounds of "threshold-many starts, then one handshake":
        // without the reset the second round would exceed CRASH_LOOP_THRESHOLD
        // and sleep for seconds.
        for _ in 0..2 {
            for _ in 0..CRASH_LOOP_THRESHOLD {
                crash_loop_backoff("test-handshake");
            }
            reset_crash_loop("test-handshake");
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "healthy start/handshake cycles must never trigger the backoff sleep"
        );
    }

    #[test]
    fn crash_loop_log_only_keeps_recent_entries() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvVarGuard::set("SOBRE_DATA_DIR", dir.path());

        let log_path = dir.path().join(".test-prune-starts.log");
        let old_ts = 1000u64;
        std::fs::write(&log_path, format!("{old_ts}\n")).unwrap();

        crash_loop_backoff("test-prune");

        let content = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "old entry should be pruned, only current remains"
        );
        let ts: u64 = lines[0].parse().unwrap();
        assert!(ts > old_ts, "remaining entry should be recent");
    }

    #[test]
    fn sanitize_lock_name_strips_special_chars() {
        assert_eq!(sanitize_lock_name("mcp-stdio"), "mcp-stdio");
        assert_eq!(sanitize_lock_name("mcp_http"), "mcp_http");
        assert_eq!(sanitize_lock_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_lock_name("name with spaces"), "name_with_spaces");
    }

    #[test]
    fn crash_loop_backoff_formula_correctness() {
        assert_eq!(
            2u64.saturating_pow(1).min(CRASH_LOOP_MAX_BACKOFF_SECS),
            2,
            "1 over threshold = 2s backoff"
        );
        assert_eq!(
            2u64.saturating_pow(2).min(CRASH_LOOP_MAX_BACKOFF_SECS),
            4,
            "2 over threshold = 4s backoff"
        );
        assert_eq!(
            2u64.saturating_pow(3).min(CRASH_LOOP_MAX_BACKOFF_SECS),
            8,
            "3 over threshold = 8s backoff"
        );
        assert_eq!(
            2u64.saturating_pow(4).min(CRASH_LOOP_MAX_BACKOFF_SECS),
            16,
            "4 over threshold = 16s backoff"
        );
        assert_eq!(
            2u64.saturating_pow(5).min(CRASH_LOOP_MAX_BACKOFF_SECS),
            30,
            "5 over threshold = capped at 30s"
        );
        assert_eq!(
            2u64.saturating_pow(10).min(CRASH_LOOP_MAX_BACKOFF_SECS),
            30,
            "10 over threshold = still capped at 30s"
        );
    }
}
