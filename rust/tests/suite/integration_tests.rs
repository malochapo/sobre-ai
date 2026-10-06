use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A config dir whose `config.toml` sets `shell_security = "warn"`. These tests
/// exercise CLI/compression behavior, not the shell allowlist (which has its
/// own unit tests in shell::exec). On CI stderr is not a TTY, so the gate would
/// block test scripts (`for`/`while` loops are not allowlisted) with exit 126 —
/// warn-only keeps the tests pinned to what they actually assert.
fn warn_only_config_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "lean-ctx-it-warn-only-config-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create warn-only config dir");
        std::fs::write(dir.join("config.toml"), "shell_security = \"warn\"\n")
            .expect("write warn-only config.toml");
        dir
    })
}

fn lean_ctx_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lean-ctx"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd.env("LEAN_CTX_ACTIVE", "1");
    cmd.env("__LEAN_CTX_SKIP_EVENTS", "1");
    cmd.env("LEAN_CTX_CONFIG_DIR", warn_only_config_dir());
    cmd
}

#[test]
fn binary_prints_help() {
    let output = lean_ctx_bin()
        .arg("--help")
        .output()
        .expect("failed to run lean-ctx");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("local context tools for coding agents"),
        "help should contain tagline"
    );
    assert!(stdout.contains("lean-ctx"), "help should mention lean-ctx");
}

#[test]
fn shell_hook_compresses_echo() {
    let output = lean_ctx_bin()
        .args(["-c", "echo", "hello", "world"])
        .output()
        .expect("failed to run lean-ctx -c");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "shell hook should pass through echo output"
    );
}

#[test]
fn help_shows_escape_hatches() {
    let output = lean_ctx_bin()
        .arg("--help")
        .output()
        .expect("failed to run lean-ctx");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("lean-ctx raw"),
        "help should document `lean-ctx raw`"
    );
    assert!(
        stdout.contains("lean-ctx-off"),
        "help should document `lean-ctx-off`"
    );
}

// ── Pipe Guard Tests ────────────────────────────────────────

#[test]
fn pipe_guard_no_compression_when_stdout_is_piped() {
    if cfg!(windows) {
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_lean-ctx"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("__LEAN_CTX_SKIP_EVENTS", "1")
        .args(["-c", "echo hello world"])
        .output()
        .expect("failed to run lean-ctx -c with piped stdout");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "hello world",
        "piped stdout must pass through raw output, got: {stdout}"
    );
}

#[test]
fn pipe_guard_force_compress_overrides_pipe_guard() {
    if cfg!(windows) {
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_lean-ctx"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("__LEAN_CTX_SKIP_EVENTS", "1")
        .args(["-c", "echo hello world"])
        .output()
        .expect("failed to run lean-ctx -c with piped stdout");
    assert!(
        output.status.success(),
        "`-c` should not crash even with piped stdout"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "output should contain the echoed text"
    );
}

#[test]
fn pipe_guard_multiline_output_unchanged_when_piped() {
    if cfg!(windows) {
        return;
    }
    let script = "echo line1; echo line2; echo line3; echo 'result: 42'";
    let output = Command::new(env!("CARGO_BIN_EXE_lean-ctx"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("__LEAN_CTX_SKIP_EVENTS", "1")
        .args(["-c", script])
        .output()
        .expect("failed to run lean-ctx -c with multiline output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("line1"), "must contain line1");
    assert!(stdout.contains("line2"), "must contain line2");
    assert!(stdout.contains("line3"), "must contain line3");
    assert!(
        stdout.contains("result: 42"),
        "must preserve exact output content"
    );
}

// ── Redirect-to-file fidelity under FORCED compression ──────
// Regression: the agent shell hook deliberately bypasses the `[ ! -t 1 ]` pipe
// guard (an agent IS the consumer), so an agent running
// `git status --short > files.txt` reached `lean-ctx -c` with stdout pointing at a
// regular FILE and forced compression on. The compressed digest was written INTO
// the file, dropping/abbreviating lines and producing contradictory diffs. Output
// redirected to a regular file must ALWAYS stay byte-faithful, even when
// compression is forced; pipes (agent capture) must keep compressing.

// >30 distinct lines so the compression engine definitely rewrites the output
// (the truncate-with-safety-scan fallback collapses the middle). This makes the
// assertions below meaningful — pre-fix, the file held the compressed digest.
// The payload must NOT contain any passthrough/verbatim keyword (notably the
// literal "lean-ctx", which classifies the whole command as Passthrough), or the
// output would never be compressed and the test would be vacuous.
fn redirect_fidelity_command() -> &'static str {
    "i=0; while [ $i -lt 60 ]; do echo \"row $i alpha beta gamma delta payload\"; i=$((i+1)); done"
}

/// `lean-ctx -c` with a *hermetic* control environment for the compression
/// fidelity tests. CI may set `LEAN_CTX_ACTIVE`; stripping it pins the exact
/// forced-compress path. `LEAN_CTX_WRAPPED` must go too: running `cargo test` *through* the lean-ctx
/// wrapper stamps it on every child, and the nested `lean-ctx -c` under test
/// would then (correctly) defer to the parent instead of compressing.
fn forced_compress_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lean-ctx"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("LEAN_CTX_ACTIVE")
        .env_remove("LEAN_CTX_WRAPPED")
        .env("__LEAN_CTX_SKIP_EVENTS", "1")
        // Allowlist enforcement (non-TTY stderr ⇒ block) is out of scope here;
        // the fidelity scripts use `while` loops that are not allowlisted.
        .env("LEAN_CTX_CONFIG_DIR", warn_only_config_dir());
    cmd
}

#[test]
fn force_compress_redirect_to_file_is_byte_faithful() {
    if cfg!(windows) {
        return;
    }
    let cmd = redirect_fidelity_command();

    // Ground truth: the raw command output (no lean-ctx in the loop).
    let raw = Command::new("sh")
        .args(["-c", cmd])
        .output()
        .expect("failed to run raw command");
    let raw_stdout = String::from_utf8_lossy(&raw.stdout).to_string();

    // Run lean-ctx with forced compression, stdout redirected to a real file —
    // exactly the agent `cmd > file` scenario.
    let tmp = std::env::temp_dir().join(format!(
        "lean-ctx-redirect-fidelity-{}.txt",
        std::process::id()
    ));
    let file = std::fs::File::create(&tmp).expect("create temp file");
    let status = forced_compress_bin()
        .args(["-c", cmd])
        .stdout(std::process::Stdio::from(file))
        .status()
        .expect("failed to run lean-ctx -c with file redirect");
    assert!(status.success(), "lean-ctx -c should exit cleanly");

    let file_content = std::fs::read_to_string(&tmp).expect("read temp file");
    let _ = std::fs::remove_file(&tmp);

    assert_eq!(
        file_content, raw_stdout,
        "redirect to a regular file under `-c` must be byte-faithful (never compressed)"
    );
}

#[test]
fn force_compress_pipe_still_compresses() {
    // Companion to the redirect test: proves compression is genuinely active for
    // the SAME command + env when stdout is a PIPE (an agent's capture). If this
    // ever stops compressing, the redirect test above would be vacuous.
    if cfg!(windows) {
        return;
    }
    let cmd = redirect_fidelity_command();

    let raw = Command::new("sh")
        .args(["-c", cmd])
        .output()
        .expect("failed to run raw command");
    let raw_stdout = String::from_utf8_lossy(&raw.stdout).to_string();

    let piped = forced_compress_bin()
        .args(["-c", cmd])
        .output()
        .expect("failed to run lean-ctx -c with piped stdout");
    let piped_stdout = String::from_utf8_lossy(&piped.stdout).to_string();

    assert_ne!(
        piped_stdout, raw_stdout,
        "piped stdout under `-c` should be compressed (changed) — proves the redirect guard is meaningful"
    );
}

// ── Extended CLI Smoke Tests ────────────────────────────────

#[test]
fn binary_gain_json_returns_valid_json() {
    let output = lean_ctx_bin()
        .args(["gain", "--json"])
        .output()
        .expect("failed to run lean-ctx gain --json");
    assert!(
        output.status.success(),
        "gain --json should succeed: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(stdout.trim());
    assert!(
        parsed.is_ok(),
        "gain --json should return valid JSON, got: {stdout}"
    );
}

#[test]
fn binary_grep_finds_matches() {
    let output = lean_ctx_bin()
        .args(["grep", "fn main", "src/"])
        .output()
        .expect("failed to run lean-ctx grep");
    assert!(
        output.status.success(),
        "grep should succeed (exit={})",
        output.status.code().unwrap_or(-1)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("main") || stdout.contains("fn"),
        "grep should find matches: {stdout}"
    );
}

#[test]
fn binary_ls_shows_tree() {
    let output = lean_ctx_bin()
        .args(["ls", "--depth", "2"])
        .output()
        .expect("failed to run lean-ctx ls");
    assert!(
        output.status.success(),
        "ls should succeed (exit={})",
        output.status.code().unwrap_or(-1)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("src") || stdout.contains("Cargo"),
        "ls should show directory tree: {stdout}"
    );
}

#[test]
fn binary_doctor_runs_without_crash() {
    let output = lean_ctx_bin()
        .arg("doctor")
        .output()
        .expect("failed to run lean-ctx doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("lean-ctx")
            || stdout.contains("doctor")
            || stdout.contains("✓")
            || stdout.contains("check"),
        "doctor should produce diagnostic output: {stdout}"
    );
}

#[test]
fn pipe_guard_rust_side_defense_in_depth() {
    if cfg!(windows) {
        return;
    }
    let script = "for i in 1 2 3 4 5; do echo \"item_$i: $(date +%s)\"; done";
    let output = Command::new(env!("CARGO_BIN_EXE_lean-ctx"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("__LEAN_CTX_SKIP_EVENTS", "1")
        // `for` + command substitution are not allowlisted; this test pins the
        // pipe guard, not enforcement (covered by shell::exec unit tests).
        .env("LEAN_CTX_CONFIG_DIR", warn_only_config_dir())
        .args(["-c", script])
        .output()
        .expect("failed to run lean-ctx -c");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for i in 1..=5 {
        assert!(
            stdout.contains(&format!("item_{i}:")),
            "Rust-side pipe guard must pass through all lines unchanged (missing item_{i})"
        );
    }
}
