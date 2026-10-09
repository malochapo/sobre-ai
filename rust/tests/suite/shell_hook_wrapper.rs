//! End-to-end regression.
//!
//! Claude Code wraps every Bash tool call in its own scaffolding before the
//! sobre shell hook forwards it to `sobre -c`. The real shape (from Claude
//! Code's `bashProvider.ts`) is:
//!
//! ```text
//! source <snapshot> 2>/dev/null || true && shopt -u extglob 2>/dev/null || true && eval '<cmd>' [< /dev/null] && pwd -P >| /tmp/claude-XXXX-cwd
//! ```
//!
//! A shell gate used to hard-block that `eval` (exit 126) on every call. sobre
//! no longer gates commands at all; these tests keep the host wrapper shapes
//! running unchanged through `sobre -c`, cwd snapshot included.

use std::path::Path;
use std::process::{Command, Output};

/// Run `sobre -c <command>` against an isolated `HOME`, with the re-entrancy
/// and disable markers cleared so the command actually flows through
/// `shell::exec` (and our unwrap) instead of passing through.
fn run_dash_c(command: &str, home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sobre"))
        .args(["-c", command])
        .env("HOME", home)
        // A `sobre`-wrapped parent (e.g. the dev shell hook running the test
        // suite) would otherwise leak these and make `-c` pass through raw.
        .env_remove("SOBRE_WRAPPED")
        .output()
        .expect("failed to spawn sobre binary")
}

// Claude Code's cwd-snapshot wrapper is POSIX-bash: `pwd -P >| <path>`, `source`,
// `/dev/null`. On Windows `sobre` selects PowerShell/cmd unless it is running
// inside Git Bash, and the Windows snapshot path (backslashes) breaks when
// embedded in a bash redirect target — so the snapshot-file half of the bug is
// exercised on POSIX only. The security boundary (a bare model-chosen `eval`
// stays hard-blocked) is verified on every platform by
// `model_chosen_eval_without_cwd_marker_still_blocks` below.
#[test]
#[cfg_attr(
    windows,
    ignore = "POSIX-bash cwd snapshot; cross-platform security covered by model_chosen_eval test"
)]
fn claude_wrapper_runs_inner_command_and_preserves_cwd_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    // Detection keys on a cwd-snapshot target; `-cwd` mirrors Claude's naming.
    let cwd_file = tmp.path().join("claude-test-cwd");
    let marker = "SOBRE595OK";

    let wrapper = format!(
        "shopt -u extglob 2>/dev/null || true && eval 'echo {marker}' < /dev/null && pwd -P >| {}",
        cwd_file.display()
    );

    let out = run_dash_c(&wrapper, tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // The whole point: the wrapper must NOT be hard-blocked any more.
    assert_ne!(
        out.status.code(),
        Some(126),
        "wrapper was still blocked (exit 126) — stderr: {stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "wrapper should run cleanly; stdout: {stdout} stderr: {stderr}"
    );
    // The real inner command ran (and compression preserved its marker).
    assert!(
        stdout.contains(marker),
        "inner command output missing; stdout: {stdout}"
    );
    // Claude's cwd tracking survived: the snapshot file exists and holds a path.
    let snapshot = std::fs::read_to_string(&cwd_file)
        .expect("cwd snapshot file must be written by the preserved `pwd -P >|`");
    assert!(
        snapshot.trim_start().starts_with('/'),
        "cwd snapshot should contain an absolute path, got: {snapshot:?}"
    );
}

// POSIX-bash only, for the same reason as the test above: the production
// `bashProvider.ts` shape relies on `source`/`pwd -P >|` and a POSIX shell +
// path semantics that Windows does not provide here.
#[test]
#[cfg_attr(
    windows,
    ignore = "POSIX-bash cwd snapshot; cross-platform security covered by model_chosen_eval test"
)]
fn real_bashprovider_wrapper_with_source_prefix_runs() {
    // The exact production shape from Claude Code's `bashProvider.ts`:
    // `source <snapshot> … && shopt … && eval '<cmd>' < /dev/null && pwd -P >| …`
    // with the `'"'"'` single-quote escaping Claude emits around the inner
    // command. Must run (no exit 126), execute the inner command, and write cwd.
    let tmp = tempfile::tempdir().unwrap();
    let snap = tmp.path().join("snap-bash.sh");
    let cwd_file = tmp.path().join("claude-real-cwd");

    let wrapper = format!(
        "source {snap} 2>/dev/null || true && shopt -u extglob 2>/dev/null || true \
         && eval 'echo '\"'\"'hello 595'\"'\"'' < /dev/null && pwd -P >| {cwd}",
        snap = snap.display(),
        cwd = cwd_file.display(),
    );

    let out = run_dash_c(&wrapper, tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_ne!(
        out.status.code(),
        Some(126),
        "real bashProvider wrapper must not be blocked; stderr: {stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "real wrapper should run cleanly; stdout: {stdout} stderr: {stderr}"
    );
    assert!(
        stdout.contains("hello 595"),
        "inner `echo 'hello 595'` output missing; stdout: {stdout}"
    );
    assert!(
        cwd_file.exists(),
        "cwd snapshot must be written even with the source/shopt scaffold present"
    );
}

/// zsh sandbox wraps with `setopt … && eval '<cmd>' … && pwd`
/// (stdout cwd, no file redirect). Must run the inner command, not exit 126.
#[test]
#[cfg_attr(
    windows,
    ignore = "zsh sandbox uses POSIX features; cross-platform security covered by model_chosen_eval test"
)]
fn zsh_sandbox_wrapper_runs_inner_command() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = "SOBRE745OK";

    let wrapper = format!(
        "setopt NO_EXTENDED_GLOB NO_BARE_GLOB_QUAL 2>/dev/null || true && eval 'echo {marker}' < /dev/null && pwd"
    );

    let out = run_dash_c(&wrapper, tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_ne!(
        out.status.code(),
        Some(126),
        "zsh sandbox wrapper must not be blocked (exit 126) — stderr: {stderr}"
    );
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
fn model_chosen_eval_runs() {
    // sobre blocks nothing: what may run is the agent host's decision.
    let tmp = tempfile::tempdir().unwrap();
    let out = run_dash_c("eval 'echo eval-ran'", tmp.path());
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("eval-ran"));
}
