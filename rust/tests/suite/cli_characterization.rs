//! CLI characterization tests — spawn the real binary with each subcommand and
//! assert stable exit codes + output invariants. These freeze the existing
//! behavior so that future refactors of `cli/dispatch` can't silently regress.

use std::process::{Command, Output};

fn lean_ctx() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lean-ctx"));
    cmd.env("HOME", "/tmp/lean-ctx-cli-test");
    cmd
}

fn run(args: &[&str]) -> Output {
    lean_ctx()
        .args(args)
        .output()
        .expect("failed to spawn lean-ctx binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

// ═══════════════════════════════════════════════════════════════════
// Basic entry points
// ═══════════════════════════════════════════════════════════════════

#[test]
fn version_flag_exits_zero_and_prints_version() {
    let out = run(&["--version"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("lean-ctx"), "expected 'lean-ctx' in: {s}");
}

#[test]
fn version_short_flag() {
    let out = run(&["-V"]);
    assert_eq!(exit_code(&out), 0);
    assert!(stdout(&out).contains("lean-ctx"));
}

#[test]
fn help_flag_exits_zero_and_lists_commands() {
    let out = run(&["--help"]);
    assert_eq!(exit_code(&out), 0);
    let s = stdout(&out);
    assert!(
        s.contains("COMMANDS") || s.contains("Usage") || s.contains("lean-ctx"),
        "help must show usage info; got: {}",
        &s[..s.len().min(200)]
    );
}

#[test]
fn help_short_flag() {
    let out = run(&["-h"]);
    assert_eq!(exit_code(&out), 0);
    assert!(!stdout(&out).is_empty());
}

/// Commands the minimal CLI dropped answer like any unknown command; `exec`
/// was the alias of `-c`, `status` folded into `doctor`, `stop` and `restart`
/// into `proxy`.
#[test]
fn removed_commands_are_unknown() {
    for cmd in [
        "read",
        "grep",
        "ls",
        "status",
        "stop",
        "restart",
        "exec",
        "session",
        "knowledge",
        "benchmark",
        "completions",
    ] {
        let out = run(&[cmd, "x"]);
        assert_eq!(exit_code(&out), 1, "{cmd} must be unknown");
        assert!(
            stderr(&out).contains("unknown command"),
            "{cmd}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn unknown_command_exits_nonzero() {
    let out = run(&["this-command-does-not-exist-xyz"]);
    assert_ne!(exit_code(&out), 0, "unknown command should fail");
    let err = stderr(&out);
    assert!(
        err.contains("unknown command") || err.contains("this-command-does-not-exist"),
        "stderr should mention the unknown command; got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Info/read-only subcommands (no side effects)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn gain_exits_zero() {
    let out = run(&["gain"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
}

#[test]
fn gain_json_exits_zero() {
    let out = run(&["gain", "--json"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.starts_with('{') || s.starts_with('[') || s.contains('"'),
        "--json should produce JSON-like output; got: {}",
        &s[..s.len().min(100)]
    );
}

#[test]
fn gain_breakdowns_exit_zero() {
    for flag in ["--by-tool", "--daily", "--period=7d"] {
        let out = run(&["gain", flag]);
        assert_eq!(exit_code(&out), 0, "gain {flag}: {}", stderr(&out));
    }
}

#[test]
fn gain_rejects_an_unknown_period() {
    let out = run(&["gain", "--period=week"]);
    assert_eq!(exit_code(&out), 2, "stderr: {}", stderr(&out));
}

#[test]
fn gain_reset_exits_zero() {
    let out = run(&["gain", "--reset"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
}

#[test]
fn config_show_exits_zero() {
    let out = run(&["config", "show"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
}

#[test]
fn config_no_args_exits_zero() {
    let out = run(&["config"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
}

#[test]
fn config_path_prints_the_file() {
    let out = run(&["config", "path"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).trim_end().ends_with("config.toml"));
}

#[test]
fn config_rejects_removed_subcommands() {
    for sub in ["validate", "schema", "apply", "effective", "which"] {
        let out = run(&["config", sub]);
        assert_eq!(exit_code(&out), 1, "config {sub}");
        assert!(
            stderr(&out).contains("Usage"),
            "config {sub}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn index_without_subcommand_prints_usage() {
    let out = run(&["index"]);
    let err = stderr(&out);
    assert!(
        err.contains("index <build|status>"),
        "index usage must name build and status: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Doctor
// ═══════════════════════════════════════════════════════════════════

#[test]
fn doctor_runs_without_panic() {
    let out = run(&["doctor"]);
    let code = exit_code(&out);
    assert!(
        code == 0 || code == 1,
        "doctor should exit 0 or 1, got {code}"
    );
    let s = stdout(&out);
    assert!(
        s.contains('✓') || s.contains('✗'),
        "doctor output should contain check marks"
    );
    // The summary `status` used to print, and where the debug log goes.
    assert!(s.contains("agents connected"), "doctor summary: {s}");
    assert!(s.contains("debug log"), "doctor debug-log path: {s}");
}

// ═══════════════════════════════════════════════════════════════════
// Subcommands that print usage on empty/wrong args
// ═══════════════════════════════════════════════════════════════════

#[test]
fn hook_no_args_prints_usage() {
    let out = run(&["hook"]);
    assert_ne!(exit_code(&out), 0);
    let err = stderr(&out);
    assert!(
        err.contains("Usage") || err.contains("lean-ctx hook"),
        "hook without subcommand should show usage"
    );
}

#[test]
fn hook_unknown_prints_usage() {
    let out = run(&["hook", "nonexistent"]);
    assert_ne!(exit_code(&out), 0);
}

// ═══════════════════════════════════════════════════════════════════
// Shell exec (-c)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn exec_echo_exits_zero() {
    let out = run(&["-c", "echo hello"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("hello"),
        "exec should pass through command output"
    );
}

#[test]
fn exec_false_exits_nonzero() {
    let out = run(&["-c", "false"]);
    assert_ne!(
        exit_code(&out),
        0,
        "exec of 'false' should propagate exit code"
    );
}

#[test]
fn raw_passes_output_through() {
    let out = run(&["raw", "echo raw_test"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("raw_test"));
}

#[test]
fn raw_no_args_prints_usage() {
    let out = run(&["raw"]);
    assert_ne!(exit_code(&out), 0);
    let err = stderr(&out);
    assert!(
        err.contains("Usage") && err.contains("raw"),
        "raw without args should show usage: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Proxy (read-only status check)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn proxy_help_lists_stop_and_restart() {
    let out = run(&["proxy", "--help"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("stop") && s.contains("restart") && s.contains("cleanup"));
}

#[test]
fn proxy_status_exits_cleanly() {
    let out = run(&["proxy", "status"]);
    let code = exit_code(&out);
    assert!(
        code == 0 || code == 1,
        "proxy status should not crash, got {code}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Uninstall (dry-run only — safe)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn uninstall_dry_run_exits_zero() {
    let out = run(&["uninstall", "--dry-run"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("dry") || s.contains("Would") || s.contains("lean-ctx") || !s.is_empty(),
        "uninstall --dry-run should describe what it would do"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Machine-facing entry points
// ═══════════════════════════════════════════════════════════════════

/// `-c` is the wire form the hook rewriter (`hook_handlers/rewrite.rs`) emits as
/// `<binary> -c "<command>"`. Assert the dispatcher still knows it.
#[test]
fn dash_c_is_not_an_unknown_command() {
    let out = run(&["-c", "echo hello"]);
    let err = stderr(&out);
    assert!(
        !err.contains("unknown command"),
        "`-c` must dispatch (the hook rewriter emits it), got stderr: {err}"
    );
}
