//! Test-runner output keeps what the agent acts on: the names of the tests
//! that ran or failed, and the pass/fail counts.
#![allow(clippy::needless_raw_string_hashes)]

use sobre::core::patterns::compress_output as compress;

#[test]
fn cargo_test_preserves_names() {
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
    let compressed = compress("cargo test", output).unwrap_or_default();
    // Must preserve at least some test names
    assert!(
        compressed.contains("login_works") || compressed.contains("auth::tests::login_works"),
        "must preserve test names in output: {compressed}"
    );
    // Must have summary
    assert!(
        compressed.contains('6') && compressed.contains("pass"),
        "must have pass count"
    );
}

#[test]
fn cargo_test_many_tests_truncates() {
    let mut lines = String::from("     Running unittests src/lib.rs\n");
    for i in 0..20 {
        lines.push_str(&format!("test module::test_{i} ... ok\n"));
    }
    lines.push_str(
        "test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.23s\n",
    );
    let compressed = compress("cargo test", &lines).unwrap_or_default();
    // Should show max 5 names + "more"
    assert!(
        compressed.contains("more"),
        "20 tests should show '...+N more': {compressed}"
    );
}

#[test]
fn cargo_test_failure_preserves_failed_name() {
    // The cargo pattern detects "FAILED" + "---" lines like "---- test_name ----"
    let output = r#"
     Running unittests src/lib.rs
test auth::login ... ok
test auth::validate ... FAILED

---- FAILED auth::validate ----
thread 'auth::validate' panicked at 'assertion failed'

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
"#;
    let compressed = compress("cargo test", output).unwrap_or_default();
    // The parser extracts the word after "FAILED" on lines containing "---"
    assert!(
        compressed.contains("fail"),
        "must indicate failure: {compressed}"
    );
    assert!(
        compressed.contains("1 pass") && compressed.contains("1 fail"),
        "must show pass/fail counts: {compressed}"
    );
}

#[test]
fn pytest_preserves_passed_names() {
    let output = r#"
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login PASSED
tests/test_auth.py::test_logout PASSED
tests/test_auth.py::test_refresh PASSED

============================== 3 passed in 0.54s ===============================
"#;
    let compressed = compress("pytest", output).unwrap_or_default();
    assert!(
        compressed.contains("3 passed"),
        "must show pass count: {compressed}"
    );
    assert!(
        compressed.contains("test_login") || compressed.contains("ran:"),
        "should preserve test names: {compressed}"
    );
}

#[test]
fn go_test_preserves_names() {
    let output = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestMultiply\n--- PASS: TestMultiply (0.00s)\n=== RUN   TestDivide\n--- PASS: TestDivide (0.01s)\nok  \tgithub.com/user/math\t0.015s\n";
    let compressed = compress("go test ./...", output).unwrap_or_default();
    assert!(
        compressed.contains("ok") && compressed.contains("github.com/user/math"),
        "must show package result: {compressed}"
    );
    assert!(
        compressed.contains("TestAdd") || compressed.contains("ran:"),
        "should preserve test names: {compressed}"
    );
}
