//! Invariant: `try_shared_engine` must never trigger a model load.
//!
//! This can only be asserted in a FRESH process. The shared engine is a
//! process-wide `OnceLock`; inside the unit-test suite any sibling test that
//! legitimately loads the engine (or the background activation thread)
//! may initialize it first, making the assertion order-dependent — that was
//! the CI flake of 2026-06-11. Integration-test binaries run in their own
//! process, so the lock here is guaranteed untouched. Keep this file
//! single-test for that reason.
#![cfg(feature = "dense")]

#[test]
fn try_shared_engine_returns_none_when_not_initialized() {
    assert!(
        sobre::core::embeddings::try_shared_engine().is_none(),
        "try_shared_engine must return None without triggering a load"
    );
}
