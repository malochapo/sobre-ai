//! Integration tests for the self-healing I/O protection layer.
//! Tests lock timeout behavior, adaptive escalation, graceful degradation,
//! and environment detection under realistic conditions.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Scenario 1: bounded_lock returns None when lock is held
// ---------------------------------------------------------------------------

/// Base timeout for the held-lock scenarios. The production base is 10 s; the
/// timeout path is the same at any base, so these wait 0.2 s instead.
const TEST_BASE: Duration = Duration::from_millis(200);

#[tokio::test]
async fn bounded_read_returns_none_when_write_held() {
    let lock: Arc<RwLock<u32>> = Arc::new(RwLock::new(42));

    // Hold write lock in background
    let _guard = lock.write().await;

    // bounded_lock::read should timeout and return None (not hang forever)
    let start = Instant::now();
    let result = tokio::task::spawn_blocking({
        let lock = lock.clone();
        move || sobre::server::bounded_lock::read_within(&lock, "test:write_held", TEST_BASE)
    })
    .await
    .unwrap();

    assert!(
        result.is_none(),
        "should return None when write lock is held"
    );
    let elapsed = start.elapsed();
    // adaptive_timeout may double the base in degraded mode
    assert!(
        elapsed < Duration::from_secs(5),
        "should not hang indefinitely, elapsed: {elapsed:?}"
    );
}

#[tokio::test]
async fn bounded_write_returns_none_when_read_held() {
    let lock: Arc<RwLock<u32>> = Arc::new(RwLock::new(42));

    // Hold read lock
    let _guard = lock.read().await;

    let start = Instant::now();
    let result = tokio::task::spawn_blocking({
        let lock = lock.clone();
        move || sobre::server::bounded_lock::write_within(&lock, "test:read_held", TEST_BASE)
    })
    .await
    .unwrap();

    assert!(
        result.is_none(),
        "should return None when read lock is held"
    );
    let elapsed = start.elapsed();
    // adaptive_timeout may double the base in degraded mode
    assert!(
        elapsed < Duration::from_secs(5),
        "should not hang, elapsed: {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 2: bounded_lock succeeds immediately when uncontended
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bounded_read_succeeds_when_free() {
    let lock: Arc<RwLock<String>> = Arc::new(RwLock::new("hello".to_string()));

    let result = tokio::task::spawn_blocking({
        let lock = lock.clone();
        move || sobre::server::bounded_lock::read(&lock, "test:free")
    })
    .await
    .unwrap();

    assert!(result.is_some(), "should succeed when lock is free");
    assert_eq!(*result.unwrap(), "hello");
}

#[tokio::test]
async fn bounded_write_succeeds_when_free() {
    let lock: Arc<RwLock<u32>> = Arc::new(RwLock::new(0));

    let result = tokio::task::spawn_blocking({
        let lock = lock.clone();
        move || sobre::server::bounded_lock::write(&lock, "test:free_write")
    })
    .await
    .unwrap();

    assert!(result.is_some(), "should succeed when lock is free");
    let mut guard = result.unwrap();
    *guard = 99;
    assert_eq!(*guard, 99);
}

// ---------------------------------------------------------------------------
// Scenario 3: Freeze counter escalation and decay
// ---------------------------------------------------------------------------

#[test]
fn freeze_counter_escalates_to_degraded() {
    use sobre::core::io_health;

    // Record multiple freezes to trigger degraded mode
    for _ in 0..5 {
        io_health::record_freeze();
    }

    let count = io_health::recent_freeze_count();
    assert!(count >= 3, "should have 3+ freezes, got {count}");

    let base = Duration::from_secs(10);
    let adapted = io_health::adaptive_timeout(base);
    // In degraded mode, timeout should be LONGER to avoid a death spiral
    assert!(
        adapted > base,
        "adapted ({adapted:?}) should be greater than base ({base:?}) in degraded mode"
    );
}

// ---------------------------------------------------------------------------
// Scenario 4: WSL2 detection (safe to run on non-WSL)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Scenario 5: slow_mount detection
// ---------------------------------------------------------------------------

#[test]
fn local_paths_not_slow() {
    use sobre::core::io_health;

    assert!(!io_health::is_slow_mount("/home/user/project/src/main.rs"));
    assert!(!io_health::is_slow_mount("/tmp/test.txt"));
    assert!(!io_health::is_slow_mount("/usr/local/bin/sobre"));
    assert!(!io_health::is_slow_mount("/var/log/syslog"));
}

#[test]
fn mnt_paths_slow_only_on_wsl() {
    use sobre::core::io_health;

    let result = io_health::is_slow_mount("/mnt/c/Users/test/project");
    if io_health::is_wsl() {
        assert!(result, "/mnt/c/ should be slow on WSL");
    } else {
        // On non-WSL, /mnt/ is not automatically slow
        // (it could be on NFS but that's detected separately)
        assert!(!result, "/mnt/ should not be slow on non-WSL");
    }
}

// ---------------------------------------------------------------------------
// Scenario 6: canonicalize_bounded doesn't hang on non-existent paths
// ---------------------------------------------------------------------------

#[test]
fn canonicalize_bounded_handles_nonexistent_path() {
    let path = std::path::Path::new("/this/path/absolutely/does/not/exist/xyzzy123");
    let start = Instant::now();
    let result = sobre::core::pathutil::safe_canonicalize_bounded(path, 2000);
    let elapsed = start.elapsed();

    // Should return the original path (since it doesn't exist)
    assert_eq!(result, path.to_path_buf());
    // Should complete quickly (not wait for timeout)
    assert!(
        elapsed < Duration::from_secs(3),
        "nonexistent path should resolve quickly, took {elapsed:?}"
    );
}

#[test]
fn canonicalize_bounded_resolves_existing_path() {
    let tmp = std::env::temp_dir();
    let start = Instant::now();
    let result = sobre::core::pathutil::safe_canonicalize_bounded(&tmp, 2000);
    let elapsed = start.elapsed();

    assert!(result.exists(), "resolved path should exist: {result:?}");
    assert!(
        elapsed < Duration::from_secs(2),
        "existing path should resolve fast, took {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 7: Multiple concurrent lock attempts don't deadlock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_bounded_reads_dont_deadlock() {
    let lock: Arc<RwLock<Vec<u32>>> = Arc::new(RwLock::new(vec![1, 2, 3]));
    let mut handles = Vec::new();

    for i in 0..10 {
        let lock = lock.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            let guard = sobre::server::bounded_lock::read(&lock, &format!("concurrent:{i}"));
            guard.map(|g| g.len())
        }));
    }

    let start = Instant::now();
    for h in handles {
        let result = h.await.unwrap();
        assert_eq!(result, Some(3));
    }
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "concurrent reads should complete fast"
    );
}

#[tokio::test]
async fn mixed_read_write_with_timeout_doesnt_deadlock() {
    let lock: Arc<RwLock<u32>> = Arc::new(RwLock::new(0));
    let mut handles = Vec::new();

    // Spawn writer that holds lock for 2s
    let lock_clone = lock.clone();
    handles.push(tokio::task::spawn_blocking(move || {
        if let Some(mut guard) = sobre::server::bounded_lock::write(&lock_clone, "slow_writer") {
            *guard += 1;
            std::thread::sleep(Duration::from_secs(2));
        }
        "writer done"
    }));

    // Give writer time to acquire lock
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Spawn readers that should timeout (lock held by writer)
    for i in 0..3 {
        let lock_clone = lock.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            let result = sobre::server::bounded_lock::read(&lock_clone, &format!("reader:{i}"));
            if result.is_some() {
                "read ok"
            } else {
                "read timeout (expected)"
            }
        }));
    }

    let start = Instant::now();
    for h in handles {
        let _ = h.await.unwrap();
    }
    // Should complete within adaptive timeout, NOT hang
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "mixed scenario should complete within bounds"
    );
}

// ---------------------------------------------------------------------------
// Scenario 8: Proxy port tests
// ---------------------------------------------------------------------------

#[test]
fn proxy_default_port_returns_valid_port() {
    let port = sobre::proxy_setup::default_port();
    assert!(port >= 4444, "port should be >= 4444, got {port}");
    assert!(port < 6000, "port should be < 6000, got {port}");
}
