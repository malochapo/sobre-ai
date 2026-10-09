//! Tests verifying the lock-contention hardening (plan: `harden_ctx_read_locks`).
//! Covers `adaptive_timeout` inversion fix, `bounded_lock` behavior under load,
//! and concurrent read scenarios.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ═══════════════════════════════════════════════════════════════════════════════
// 1. adaptive_timeout: slow/degraded → LONGER, not shorter
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
// 2. bounded_lock: timeout returns None, no panic
// ═══════════════════════════════════════════════════════════════════════════════

/// Base timeout for the contention tests. The production base is 10 s; the
/// timeout path is the same at any base, so these wait 0.2 s instead.
const TEST_BASE: Duration = Duration::from_millis(200);

// ═══════════════════════════════════════════════════════════════════════════════
// 3. Parallel reads: multiple concurrent readers on same lock
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_blocks_readers_but_they_recover() {
    let lock: Arc<RwLock<u32>> = Arc::new(RwLock::new(0));

    // Writer holds lock for 1s
    let lock_w = lock.clone();
    let writer = tokio::task::spawn_blocking(move || {
        if let Some(mut guard) = sobre::server::bounded_lock::write(&lock_w, "blocking_writer") {
            *guard = 42;
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Readers that start while writer holds lock
    let mut readers = Vec::new();
    for i in 0..4 {
        let lock_r = lock.clone();
        readers.push(tokio::task::spawn_blocking(move || {
            sobre::server::bounded_lock::read(&lock_r, &format!("waiting_reader:{i}")).map(|g| *g)
        }));
    }

    writer.await.unwrap();

    for h in readers {
        let result = h.await.unwrap();
        // Readers may have gotten None (timeout) or Some(42) (after writer released)
        // Either is acceptable — the key is they don't hang or panic
        if let Some(val) = result {
            assert_eq!(val, 42);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 4. Simulated 2-agent contention on session read-lock
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// 5. Channel error delivery (slow-path thread sends error, not silent return)
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// 6. Post-dispatch spawn doesn't block caller
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// 7. Simulated multi-agent scenario (2 agents, interleaved reads + writes)
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// 8. Zombie thread scenario: cancelled thread vs new reader
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// 9. Graceful degradation: enrichment timeout doesn't lose read result
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// 10. Adaptive timeout with io_health integration
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
