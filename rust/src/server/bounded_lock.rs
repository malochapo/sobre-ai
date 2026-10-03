use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

const BASE_READ_TIMEOUT: Duration = Duration::from_secs(10);
const BASE_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Spin interval between `try_*` attempts. Kept short enough for responsiveness
/// but long enough to avoid busy-spinning on Windows where thread scheduling
/// quanta are ~15ms.
const SPIN_INTERVAL: Duration = Duration::from_millis(20);

/// Acquire a read lock via a non-blocking spin loop with an adaptive timeout.
///
/// Unlike the previous `Handle::block_on` approach, this never parks a blocking
/// thread waiting for the async runtime to make progress — eliminating the
/// stall-under-load anti-pattern on Windows (#1018). The loop yields the thread
/// between attempts so it does not starve other work on the blocking pool.
///
/// Returns `None` on timeout (caller must provide graceful fallback).
pub fn read<T: Send + Sync + 'static>(
    lock: &Arc<RwLock<T>>,
    context: &str,
) -> Option<OwnedRwLockReadGuard<T>> {
    read_within(lock, context, BASE_READ_TIMEOUT)
}

/// [`read`] with a caller-chosen base timeout, still scaled by the I/O health
/// multiplier. Lets tests exercise the timeout path without waiting 10 s.
pub fn read_within<T: Send + Sync + 'static>(
    lock: &Arc<RwLock<T>>,
    context: &str,
    base: Duration,
) -> Option<OwnedRwLockReadGuard<T>> {
    let timeout = crate::core::io_health::adaptive_timeout(base);
    let deadline = std::time::Instant::now() + timeout;

    loop {
        if let Ok(guard) = lock.clone().try_read_owned() {
            return Some(guard);
        }
        if std::time::Instant::now() >= deadline {
            crate::core::io_health::record_freeze();
            tracing::warn!(
                "bounded_lock: read timeout ({}ms) for {context}; degrading gracefully",
                timeout.as_millis()
            );
            return None;
        }
        std::thread::sleep(SPIN_INTERVAL);
    }
}

/// Acquire a write lock via a non-blocking spin loop with an adaptive timeout.
/// See `read()` for design rationale (#1018).
///
/// Returns `None` on timeout (caller must provide graceful fallback).
pub fn write<T: Send + Sync + 'static>(
    lock: &Arc<RwLock<T>>,
    context: &str,
) -> Option<OwnedRwLockWriteGuard<T>> {
    write_within(lock, context, BASE_WRITE_TIMEOUT)
}

/// [`write`] with a caller-chosen base timeout. See [`read_within`].
pub fn write_within<T: Send + Sync + 'static>(
    lock: &Arc<RwLock<T>>,
    context: &str,
    base: Duration,
) -> Option<OwnedRwLockWriteGuard<T>> {
    let timeout = crate::core::io_health::adaptive_timeout(base);
    let deadline = std::time::Instant::now() + timeout;

    loop {
        if let Ok(guard) = lock.clone().try_write_owned() {
            return Some(guard);
        }
        if std::time::Instant::now() >= deadline {
            crate::core::io_health::record_freeze();
            tracing::warn!(
                "bounded_lock: write timeout ({}ms) for {context}; degrading gracefully",
                timeout.as_millis()
            );
            return None;
        }
        std::thread::sleep(SPIN_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_succeeds_on_uncontested_lock() {
        let lock = Arc::new(RwLock::new(0u32));
        let mut guard = write(&lock, "test").expect("uncontested write must succeed");
        *guard = 7;
        assert_eq!(*guard, 7);
    }

    #[test]
    fn multiple_readers_concurrent() {
        let lock = Arc::new(RwLock::new(99u32));
        let g1 = read(&lock, "r1").expect("first reader");
        let g2 = read(&lock, "r2").expect("second reader");
        assert_eq!(*g1, 99);
        assert_eq!(*g2, 99);
    }
}
