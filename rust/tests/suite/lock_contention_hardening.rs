//! Lock contention: the adaptive timeout only ever lengthens in a slow or
//! degraded environment, and `bounded_lock` readers queued behind a writer
//! get the written value or a timeout, never a hang or a panic.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Base timeout for the contention tests. The production base is 10 s; the
/// timeout path is the same at any base, so these wait 0.2 s instead.
const TEST_BASE: Duration = Duration::from_millis(200);

// ═══════════════════════════════════════════════════════════════════════════════
// adaptive_timeout: slow/degraded → LONGER, not shorter
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn adaptive_timeout_fast_returns_base() {
    use sobre::core::io_health;
    let base = Duration::from_secs(10);
    let adapted = io_health::adaptive_timeout(base);
    assert!(
        adapted >= base,
        "fast environment should return >= base, got {adapted:?}"
    );
}

#[test]
fn adaptive_timeout_degraded_returns_longer_than_base() {
    use sobre::core::io_health;

    for _ in 0..10 {
        io_health::record_freeze();
    }

    let base = Duration::from_secs(10);
    let adapted = io_health::adaptive_timeout(base);
    assert!(
        adapted > base,
        "degraded environment MUST return longer timeout, got {adapted:?} for base {base:?}"
    );
    assert!(
        adapted <= base.mul_f32(3.0),
        "should not be excessively long, got {adapted:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Readers queued behind a writer
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_blocks_readers_but_they_recover() {
    let lock: Arc<RwLock<u32>> = Arc::new(RwLock::new(0));

    // The writer holds the lock for 1 s, and says when it has it.
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let lock_w = lock.clone();
    let writer = tokio::task::spawn_blocking(move || {
        let mut guard = sobre::server::bounded_lock::write(&lock_w, "blocking_writer")
            .expect("an uncontended write lock is granted");
        *guard = 42;
        held_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_secs(1));
    });
    held_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the writer takes the lock");

    // Readers that start while the writer holds the lock.
    let mut readers = Vec::new();
    for i in 0..4 {
        let lock_r = lock.clone();
        readers.push(tokio::task::spawn_blocking(move || {
            sobre::server::bounded_lock::read(&lock_r, &format!("waiting_reader:{i}")).map(|g| *g)
        }));
    }

    writer.await.unwrap();

    for h in readers {
        // A reader either times out (None) or sees the writer's value; it never
        // hangs, panics or reads the value from before the write.
        if let Some(val) = h.await.unwrap() {
            assert_eq!(val, 42);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Adaptive timeout with io_health integration
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn bounded_lock_respects_adaptive_timeout_in_degraded() {
    use sobre::core::io_health;

    // Push into degraded mode
    for _ in 0..10 {
        io_health::record_freeze();
    }

    let lock: Arc<RwLock<u32>> = Arc::new(RwLock::new(0));
    let _hold = lock.write().await;

    let start = Instant::now();
    let result = tokio::task::spawn_blocking({
        let lock = lock.clone();
        move || sobre::server::bounded_lock::write_within(&lock, "degraded_test", TEST_BASE)
    })
    .await
    .unwrap();

    let elapsed = start.elapsed();

    assert!(result.is_none(), "should return None under contention");
    // In degraded mode the timeout is twice the base.
    assert!(
        elapsed >= TEST_BASE * 2,
        "degraded timeout should be >= 2x base, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "should not exceed reasonable bounds, got {elapsed:?}"
    );
}
