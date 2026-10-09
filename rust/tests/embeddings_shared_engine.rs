//! `try_shared_engine` never loads the model.
//!
//! The shared engine is a process-wide `OnceLock`, so this holds only in a
//! process where nothing has touched it yet: any other test that loads the
//! engine, or the background activation it starts, would fill it first. That
//! is why this test has a binary of its own and nothing else in it does.
#![cfg(feature = "dense")]

#[path = "support/user_dirs.rs"]
mod user_dirs;

#[test]
fn harness_sandboxes_user_dirs() {
    user_dirs::assert_sandboxed();
}

#[test]
fn try_shared_engine_returns_none_when_not_initialized() {
    assert!(
        sobre::core::embeddings::try_shared_engine().is_none(),
        "try_shared_engine must return None without triggering a load"
    );
}
