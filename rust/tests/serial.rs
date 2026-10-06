//! Integration-test harness for tests that change process-global state.
//! The modules live in serial_suite/mod.rs.
//!
//! Every test in this binary runs on one thread, with or without `#[serial]`:
//! the static below runs before `main` and sets `RUST_TEST_THREADS=1`, which
//! the libtest harness reads when no `--test-threads` flag is given. Under
//! nextest, `.config/nextest.toml` puts this binary in the `serial-state`
//! group for the same reason.

#[path = "support/user_dirs.rs"]
mod user_dirs;

#[test]
fn harness_sandboxes_user_dirs() {
    user_dirs::assert_sandboxed();
}

mod serial_suite;

extern "C" fn force_single_test_thread() {
    // SAFETY: runs from the platform's initializer list before `main`, while
    // the process has a single thread.
    unsafe { std::env::set_var("RUST_TEST_THREADS", "1") };
}

#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android", target_os = "freebsd"),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static FORCE_SINGLE_TEST_THREAD: extern "C" fn() = force_single_test_thread;

#[test]
fn harness_forces_a_single_test_thread() {
    assert_eq!(
        std::env::var("RUST_TEST_THREADS").as_deref(),
        Ok("1"),
        "the pre-main initializer did not run: tests in this binary would run in parallel"
    );
}
