//! The command wrappers agent hosts put around a shell call run unchanged
//! through `sobre -c`.
//!
//! Claude Code wraps every Bash tool call in its own scaffolding before the
//! sobre shell hook forwards it to `sobre -c`. Its shape (from Claude Code's
//! `bashProvider.ts`) is:
//!
//! ```text
//! source <snapshot> 2>/dev/null || true && shopt -u extglob 2>/dev/null || true && eval '<cmd>' [< /dev/null] && pwd -P >| /tmp/claude-XXXX-cwd
//! ```
//!
//! sobre gates no command, so the inner command runs and the host's cwd
//! snapshot is written, exactly as without sobre.

use std::path::Path;
use std::process::{Command, Output};

/// Run `sobre -c <command>` against an isolated `HOME`, with the re-entrancy
/// marker cleared so the command goes through `shell::exec` instead of
/// passing through.
fn run_dash_c(command: &str, home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sobre"))
        .args(["-c", command])
        .env("HOME", home)
        // A `sobre`-wrapped parent (e.g. the dev shell hook running the test
        // suite) would otherwise leak this and make `-c` pass through raw.
        .env_remove("SOBRE_WRAPPED")
        .output()
        .expect("failed to spawn sobre binary")
}

// The wrappers below are POSIX shell: `source`, `pwd -P >|`, `/dev/null`. On
// Windows `sobre -c` runs PowerShell or cmd unless it is inside Git Bash, and
// a Windows path (backslashes) breaks as a bash redirect target.
#[test]
#[cfg_attr(windows, ignore = "the wrapper is POSIX shell")]
fn claude_wrapper_runs_inner_command_and_preserves_cwd_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    // `-cwd` mirrors the name Claude Code gives its snapshot file.
    let cwd_file = tmp.path().join("claude-test-cwd");
    let marker = "SOBRE_WRAPPER_OK";

    let wrapper = format!(
        "shopt -u extglob 2>/dev/null || true && eval 'echo {marker}' < /dev/null && pwd -P >| {}",
        cwd_file.display()
    );

    let out = run_dash_c(&wrapper, tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(0),
        "wrapper should run cleanly; stdout: {stdout} stderr: {stderr}"
    );
    assert!(
        stdout.contains(marker),
        "inner command output missing; stdout: {stdout}"
    );
    // The host's cwd tracking survives: the snapshot file holds a path.
    let snapshot = std::fs::read_to_string(&cwd_file)
        .expect("cwd snapshot file must be written by `pwd -P >|`");
    assert!(
        snapshot.trim_start().starts_with('/'),
        "cwd snapshot should contain an absolute path, got: {snapshot:?}"
    );
}

#[test]
#[cfg_attr(windows, ignore = "the wrapper is POSIX shell")]
fn real_bashprovider_wrapper_with_source_prefix_runs() {
    // The full shape from Claude Code's `bashProvider.ts`:
    // `source <snapshot> … && shopt … && eval '<cmd>' < /dev/null && pwd -P >| …`
    // with the `'"'"'` single-quote escaping Claude emits around the inner
    // command. It runs the inner command and writes the cwd.
    let tmp = tempfile::tempdir().unwrap();
    let snap = tmp.path().join("snap-bash.sh");
    let cwd_file = tmp.path().join("claude-real-cwd");

    let wrapper = format!(
        "source {snap} 2>/dev/null || true && shopt -u extglob 2>/dev/null || true \
         && eval 'echo '\"'\"'hello wrapper'\"'\"'' < /dev/null && pwd -P >| {cwd}",
        snap = snap.display(),
        cwd = cwd_file.display(),
    );

    let out = run_dash_c(&wrapper, tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(0),
        "real wrapper should run cleanly; stdout: {stdout} stderr: {stderr}"
    );
    assert!(
        stdout.contains("hello wrapper"),
        "inner `echo 'hello wrapper'` output missing; stdout: {stdout}"
    );
    assert!(
        cwd_file.exists(),
        "cwd snapshot must be written even with the source/shopt scaffold present"
    );
}

/// The zsh sandbox wraps with `setopt … && eval '<cmd>' … && pwd` (cwd on
/// stdout, no file redirect).
#[test]
#[cfg_attr(windows, ignore = "the wrapper is POSIX shell")]
fn zsh_sandbox_wrapper_runs_inner_command() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = "SOBRE_ZSH_WRAPPER_OK";

    let wrapper = format!(
        "setopt NO_EXTENDED_GLOB NO_BARE_GLOB_QUAL 2>/dev/null || true && eval 'echo {marker}' < /dev/null && pwd"
    );

    let out = run_dash_c(&wrapper, tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(0),
        "zsh sandbox wrapper should run cleanly; stdout: {stdout} stderr: {stderr}"
    );
    assert!(
        stdout.contains(marker),
        "inner command output missing; stdout: {stdout}"
    );
}

#[test]
#[cfg_attr(windows, ignore = "`eval` is a POSIX shell builtin")]
fn model_chosen_eval_runs() {
    // sobre blocks nothing: what may run is the agent host's decision.
    let tmp = tempfile::tempdir().unwrap();
    let out = run_dash_c("eval 'echo eval-ran'", tmp.path());
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("eval-ran"));
}
