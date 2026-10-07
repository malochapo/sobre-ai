use crate::core::debug_log::{self, Route};
use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

const HOOK_STDIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Hard wall-clock budget for a pre-run rewrite hook. A hang FAILS OPEN — the
/// command runs as written — instead of wedging the host's tool call (#1035).
const HOOK_GATING_TIMEOUT: Duration = Duration::from_secs(15);
// Post-run shell output compression (Claude Code, Copilot CLI, OpenCode).
mod compress;
mod dedup;
mod edit_health;
// The host's own command rules, checked before any rewrite.
mod host_rules;
mod observe;
mod payload;
mod read_dedup;
// Pre-run `lean-ctx -c` wrapping (Cursor, Gemini CLI).
mod rewrite;
pub use compress::handle_compress;
pub use observe::*;
pub use read_dedup::handle_read_dedup;
pub use rewrite::{handle_cursor, handle_gemini};

/// Mark this process as a hook child so the daemon-client never auto-starts
/// the daemon from inside a hook (which would create zombie processes).
pub fn mark_hook_environment() {
    crate::core::runtime_flags::mark_hook_child();
}

/// Arms a watchdog that force-exits the process after the given duration.
/// Prevents hook processes from becoming zombies when stdin pipes break or
/// the IDE cancels the call. Since hooks MUST NOT spawn child processes
/// (to avoid orphan zombies), a simple exit(1) suffices.
pub fn arm_watchdog(timeout: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        eprintln!(
            "[lean-ctx hook] watchdog timeout after {}s — force exit",
            timeout.as_secs()
        );
        std::process::exit(1);
    });
}

/// Run a pre-run rewrite hook's logic under a hard wall-clock timeout and
/// print the result exactly once.
///
/// On timeout the hook FAILS OPEN — it prints the pass-through output so a
/// slow or hung hook (a stalled subprocess, a wedged dedup wait, a saturated host)
/// can never block the host's tool call: the command simply runs unmodified
/// (#1035). The worker thread is abandoned on timeout (it only sends to a channel,
/// never prints, and dies with the process), so there is no double-output race —
/// `emit_gating_decision` is the single writer to stdout.
fn emit_gating_decision<F>(timeout: Duration, work: F)
where
    F: FnOnce() -> String + Send + 'static,
{
    let out = decide_with_timeout(timeout, build_passthrough_output(), work);
    print!("{out}");
}

/// Run `work` under a hard wall-clock timeout, returning `fallback` if it does not
/// finish in time. Split from [`emit_gating_decision`]'s printing so the fail-open
/// behavior is unit-testable. The worker only sends to a channel (it never prints)
/// and is abandoned on timeout, so it can never double-write the host's stdout
/// (#1035).
fn decide_with_timeout<F>(timeout: Duration, fallback: String, work: F) -> String
where
    F: FnOnce() -> String + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(timeout).unwrap_or(fallback)
}

/// Reads all of stdin with a timeout. Returns None if stdin is empty, broken, or times out.
fn read_stdin_with_timeout(timeout: Duration) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let result = std::io::stdin().read_to_string(&mut buf);
        let _ = tx.send(result.ok().map(|_| buf));
    });
    match rx.recv_timeout(timeout) {
        Ok(Some(s)) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Pass-through: no decision, no mutation. Every supported host reads `{}` as
/// "proceed as you normally would" (Cursor requires a JSON object), so the
/// host's own permission rules apply unchanged.
fn build_passthrough_output() -> String {
    "{}".to_string()
}

/// True when a host tool name denotes a shell/terminal command tool.
///
/// Copilot CLI exposes `powershell` as a first-class shell tool on Windows
/// (paired with `bash` per the CLI tool reference); without it Windows shell
/// calls would be missed (#556).
fn is_shell_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Bash"
            | "bash"
            | "Shell"
            | "shell"
            | "sh"
            | "runInTerminal"
            | "run_in_terminal"
            | "run_terminal"
            | "runterminal"
            | "run_command"
            | "run_shell_command"
            | "run_terminal_command"
            | "execute_command"
            | "exec_command"
            | "command_exec"
            | "run"
            | "exec"
            | "execute"
            | "command"
            | "cmd"
            | "terminal"
            | "PowerShell"
            | "powershell"
            | "pwsh"
    )
}

/// Resolve the lean-ctx executable path for hook command emission and
/// subprocess spawning. Always the **native** OS path: the MSYS/Git-Bash
/// `/c/...` form breaks `CreateProcess` on Windows and cannot be run by
/// PowerShell or cmd (#518). Native `C:/...` runs in PowerShell, cmd *and*
/// Git Bash, so it is the correct universal form for executed commands.
/// (MSYS `/c/...` is only needed for bash *source* lines — see `cli::shell_init`.)
fn resolve_binary() -> String {
    crate::core::portable_binary::resolve_portable_binary()
}
