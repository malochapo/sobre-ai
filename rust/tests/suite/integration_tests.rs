//! The binary end to end: help, `-c` on short and long output, output
//! redirected to a file, `gain --json` and `doctor`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A throwaway state dir, so the binary's event log never lands in the
/// developer's real one.
fn isolated_state_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("sobre-it-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create isolated state dir");
        dir
    })
}

/// An empty config dir, so the developer's own config.toml never shapes what
/// these tests see.
fn isolated_config_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("sobre-it-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create isolated config dir");
        dir
    })
}

/// The binary in the crate directory with isolated state and config, and
/// without the re-entrancy marker a `sobre`-wrapped parent (running the suite
/// through the shell hook) would leak, which would make `-c` defer to it.
fn sobre_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sobre"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("SOBRE_WRAPPED")
        .env("SOBRE_STATE_DIR", isolated_state_dir())
        .env("SOBRE_CONFIG_DIR", isolated_config_dir());
    cmd
}

#[test]
fn binary_prints_help() {
    let output = sobre_bin()
        .arg("--help")
        .output()
        .expect("failed to run sobre");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("local context tools for coding agents"),
        "help should contain tagline"
    );
    assert!(
        stdout.contains("sobre raw"),
        "help should document `sobre raw`"
    );
}

#[cfg(unix)]
#[test]
fn short_output_passes_through_unchanged() {
    let output = sobre_bin()
        .args(["-c", "echo line1 && echo line2 && echo 'result: 42'"])
        .output()
        .expect("failed to run sobre -c");
    assert!(
        output.status.success(),
        "`-c` exits with the command's code"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in ["line1", "line2", "result: 42"] {
        assert!(stdout.contains(line), "missing {line:?}: {stdout}");
    }
}

// ── Output redirected to a file stays byte-faithful ─────────
// An agent's command hook routes `cmd > files.txt` through `sobre -c` with
// stdout pointing at a regular file. Compressing there would write the digest
// into the file (dropped and abbreviated lines), so output that goes to a
// regular file is never compressed; a pipe (the agent capturing output) is.

// More than 30 distinct lines, so the compression engine rewrites the output
// when it is allowed to. The payload must not contain a passthrough keyword
// (notably the literal "sobre", which makes the whole command Passthrough), or
// it would never be compressed and the tests would prove nothing.
#[cfg(unix)]
fn long_output_command() -> &'static str {
    "i=0; while [ $i -lt 60 ]; do echo \"row $i alpha beta gamma delta payload\"; i=$((i+1)); done"
}

#[cfg(unix)]
#[test]
fn redirect_to_a_file_is_byte_faithful() {
    let cmd = long_output_command();

    // Ground truth: the raw command output (no sobre in the loop).
    let raw = Command::new("sh")
        .args(["-c", cmd])
        .output()
        .expect("failed to run raw command");
    let raw_stdout = String::from_utf8_lossy(&raw.stdout).to_string();

    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("out.txt");
    let file = std::fs::File::create(&target).expect("create temp file");
    let status = sobre_bin()
        .args(["-c", cmd])
        .stdout(std::process::Stdio::from(file))
        .status()
        .expect("failed to run sobre -c with file redirect");
    assert!(status.success(), "sobre -c should exit cleanly");

    let file_content = std::fs::read_to_string(&target).expect("read temp file");
    assert_eq!(
        file_content, raw_stdout,
        "redirect to a regular file under `-c` must be byte-faithful (never compressed)"
    );
}

/// The same command and environment through a pipe is compressed, which is
/// what makes the redirect test above meaningful.
#[cfg(unix)]
#[test]
fn piped_long_output_is_compressed() {
    let cmd = long_output_command();

    let raw = Command::new("sh")
        .args(["-c", cmd])
        .output()
        .expect("failed to run raw command");
    let raw_stdout = String::from_utf8_lossy(&raw.stdout).to_string();

    let piped = sobre_bin()
        .args(["-c", cmd])
        .output()
        .expect("failed to run sobre -c with piped stdout");
    let piped_stdout = String::from_utf8_lossy(&piped.stdout).to_string();

    assert_ne!(
        piped_stdout, raw_stdout,
        "piped stdout under `-c` should be compressed"
    );
}

#[test]
fn binary_gain_json_returns_valid_json() {
    let output = sobre_bin()
        .args(["gain", "--json"])
        .output()
        .expect("failed to run sobre gain --json");
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
