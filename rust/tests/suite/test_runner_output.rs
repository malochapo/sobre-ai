//! A test run's output, as `sobre_shell` shows it, keeps what the agent acts
//! on: the pass/fail counts and the names of the tests that failed; pytest
//! and go test also name the tests that ran.
#![allow(clippy::needless_raw_string_hashes)]

/// What `sobre_shell` shows for a test run that exited with `exit_code`.
fn shown(command: &str, output: &str, exit_code: i32) -> String {
    sobre::tools::sobre_shell::handle(command, output, exit_code, sobre::tools::CrpMode::Off)
}

#[test]
fn cargo_test_keeps_the_counts() {
    let output = r#"
   Compiling myapp v0.1.0
    Finished test target(s) in 2.34s
     Running unittests src/lib.rs
test auth::tests::login_works ... ok
test auth::tests::logout_clears_session ... ok
test auth::tests::token_refresh ... ok
test db::connection_pool_reuse ... ok
test db::migration_up ... ok
test db::migration_down ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s
"#;
    let shown = shown("cargo test", output, 0);
    assert!(shown.contains("6 passed"), "must have pass count: {shown}");
}

/// The failed test is named whether the exit code reached sobre (101) or
/// not (0, as a host hook reports it).
#[test]
fn cargo_test_failure_keeps_the_failed_name() {
    let output = r#"
     Running unittests src/lib.rs
test auth::login ... ok
test auth::validate ... FAILED

---- FAILED auth::validate ----
thread 'auth::validate' panicked at 'assertion failed'

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
"#;
    for exit_code in [0, 101] {
        let shown = shown("cargo test", output, exit_code);
        assert!(
            shown.contains("auth::validate"),
            "exit {exit_code}: must name the failed test: {shown}"
        );
        assert!(
            shown.contains("1 passed") && shown.contains("1 failed"),
            "exit {exit_code}: must show pass/fail counts: {shown}"
        );
    }
}

#[test]
fn pytest_keeps_passed_names() {
    let output = r#"
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login PASSED
tests/test_auth.py::test_logout PASSED
tests/test_auth.py::test_refresh PASSED

============================== 3 passed in 0.54s ===============================
"#;
    let shown = shown("pytest", output, 0);
    assert!(shown.contains("3 passed"), "must show pass count: {shown}");
    assert!(
        shown.contains("test_login"),
        "should name the tests that ran: {shown}"
    );
}

#[test]
fn go_test_keeps_names() {
    let output = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestMultiply\n--- PASS: TestMultiply (0.00s)\n=== RUN   TestDivide\n--- PASS: TestDivide (0.01s)\nok  \tgithub.com/user/math\t0.015s\n";
    let shown = shown("go test ./...", output, 0);
    assert!(
        shown.contains("ok") && shown.contains("github.com/user/math"),
        "must show package result: {shown}"
    );
    assert!(
        shown.contains("TestAdd"),
        "should name the tests that ran: {shown}"
    );
}
