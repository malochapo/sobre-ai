//! CLI characterization tests — spawn the real binary with each subcommand and
//! assert stable exit codes + output invariants. These freeze the existing
//! behavior so that future refactors of `cli/dispatch` can't silently regress.

use std::process::{Command, Output};

/// The binary under test. HOME and the XDG dirs are this test process's
/// sandbox (tests/support/user_dirs.rs), which the child inherits.
fn sobre() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sobre"))
}

fn run(args: &[&str]) -> Output {
    sobre()
        .args(args)
        .output()
        .expect("failed to spawn sobre binary")
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
    assert!(s.contains("sobre"), "expected 'sobre' in: {s}");
}

#[test]
fn version_short_flag() {
    let out = run(&["-V"]);
    assert_eq!(exit_code(&out), 0);
    assert!(stdout(&out).contains("sobre"));
}

#[test]
fn help_flag_exits_zero_and_lists_commands() {
    let out = run(&["--help"]);
    assert_eq!(exit_code(&out), 0);
    let s = stdout(&out);
    assert!(
        s.contains("GETTING STARTED:") && s.contains("sobre init"),
        "help must list the commands; got: {s}"
    );
}

#[test]
fn help_short_flag() {
    let out = run(&["-h"]);
    assert_eq!(exit_code(&out), 0);
    assert!(!stdout(&out).is_empty());
}

#[test]
fn unknown_command_exits_nonzero() {
    let out = run(&["this-command-does-not-exist-xyz"]);
    assert_ne!(exit_code(&out), 0, "unknown command should fail");
    let err = stderr(&out);
    assert!(
        err.contains("this-command-does-not-exist"),
        "stderr should mention the unknown command; got: {err}"
    );
}

/// A mistyped command or flag is named, with the one meant.
#[test]
fn a_typo_suggests_the_command() {
    let out = run(&["doctr"]);
    assert_eq!(exit_code(&out), 2);
    assert!(stderr(&out).contains("'doctor'"), "{}", stderr(&out));
}

/// An unknown flag is a usage error: exit 2, and the command does not run.
#[test]
fn an_unknown_flag_exits_2_and_runs_nothing() {
    for args in [
        &["gain", "--bogus"][..],
        &["gain", "--reset", "--bogus"],
        &["doctor", "--bogus"],
        &["config", "set", "proxy.port", "9000", "--bogus"],
        &["proxy", "disable", "--bogus"],
        &["init", "--bogus"],
    ] {
        let out = run(args);
        assert_eq!(exit_code(&out), 2, "{args:?}: {}", stderr(&out));
        assert!(stdout(&out).is_empty(), "{args:?} ran: {}", stdout(&out));
        assert!(
            stderr(&out).contains("--bogus"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

/// `--help` after a command prints its help and runs nothing.
#[test]
fn command_help_prints_usage_and_details() {
    let out = run(&["gain", "--help"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("Usage: sobre gain"), "{s}");
    assert!(s.contains("--period <all|today|Nd>"), "{s}");
    assert!(s.contains("usage ledger (usage.jsonl)"), "{s}");
}

#[test]
fn help_all_prints_the_full_reference() {
    let out = run(&["help", "all"]);
    assert_eq!(exit_code(&out), 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for line in [
        "sobre proxy start",
        "sobre config set <key> <value>",
        "EXIT CODES:",
    ] {
        assert!(s.contains(line), "{line}: {s}");
    }
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
fn config_rejects_an_unknown_subcommand() {
    let out = run(&["config", "bogus"]);
    assert_eq!(exit_code(&out), 2);
    assert!(
        stderr(&out).contains("Usage: sobre config"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn index_without_subcommand_prints_usage() {
    let out = run(&["index"]);
    assert_eq!(exit_code(&out), 2);
    let err = stderr(&out);
    assert!(
        err.contains("Usage: sobre index") && err.contains("build") && err.contains("status"),
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
    // The install summary, and where the debug log goes.
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
        err.contains("Usage") || err.contains("sobre hook"),
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
fn exec_propagates_the_exit_code() {
    let out = run(&["-c", "exit 3"]);
    assert_eq!(exit_code(&out), 3, "`-c` exits with the command's own code");
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
