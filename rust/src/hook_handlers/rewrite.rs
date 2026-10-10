//! `hook cursor` / `hook gemini`: wrap a shell command in `sobre -c` before
//! the host runs it, for hosts that cannot replace a command's output after
//! the fact.
//!
//! The rewrite is the only thing these hooks ever say. They never emit a
//! permission decision, so the host's own approval flow runs as usual, and they
//! only rewrite when [`host_rules::same_treatment`] confirms no rule the host
//! applies tells the original and the wrapped command apart. A command is only
//! wrapped whole, and only when it is a single command sobre knows how to
//! compress: no chains, pipes, background jobs, redirections, substitutions
//! or heredocs, and never a file read (`cat`/`head`/`tail`). The wrapped command's compressed
//! output ends with a `[sobre: …]` line saying so.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::host_rules::{self, Host};
use super::{
    HOOK_GATING_TIMEOUT, HOOK_STDIN_TIMEOUT, build_passthrough_output, dedup, emit_gating_decision,
    is_shell_tool, payload, read_stdin_with_timeout, resolve_binary,
};
use crate::compound_lexer;
use crate::core::debug_log;

pub fn handle_cursor() {
    emit_gating_decision(HOOK_GATING_TIMEOUT, || compute(Host::Cursor));
}

pub fn handle_gemini() {
    emit_gating_decision(HOOK_GATING_TIMEOUT, || compute(Host::Gemini));
}

fn compute(host: Host) -> String {
    match read_stdin_with_timeout(HOOK_STDIN_TIMEOUT) {
        Some(input) => compute_from(host, &input),
        None => build_passthrough_output(),
    }
}

fn compute_from(host: Host, input: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(input.trim_start_matches('\u{feff}')) else {
        return build_passthrough_output();
    };
    let Some(tool) = payload::resolve_tool_name(&v) else {
        return build_passthrough_output();
    };
    if !is_shell_tool(&tool) {
        return build_passthrough_output();
    }
    let args = payload::resolve_tool_args(&v);
    let Some(cmd) = payload::resolve_command(&v, args.as_ref()) else {
        return build_passthrough_output();
    };
    let cwd = payload_cwd(&v);

    // Cursor fires preToolUse twice with byte-identical payloads; the second
    // fire replays the first. The key is the whole payload, so the same
    // command run elsewhere (another `cwd`) is a call of its own.
    let key = format!("{host:?}\u{0}{input}");
    dedup::deduped("rewrite", &key, || {
        let binary = resolve_binary();
        match rewrite_for(host, &cmd, &binary, &cwd) {
            Some(wrapped) => {
                debug_log::log_hook_decision("rewrite", &tool, &cmd, "wrapped");
                reply(host, args.as_ref(), &wrapped)
            }
            None => build_passthrough_output(),
        }
    })
}

/// Where the host says the command runs: `cwd`, else the first workspace root.
fn payload_cwd(v: &Value) -> PathBuf {
    v.get("cwd")
        .and_then(Value::as_str)
        .or_else(|| {
            v.get("workspace_roots")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
        })
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

fn rewrite_for(host: Host, cmd: &str, binary: &str, cwd: &Path) -> Option<String> {
    let wrapped = candidate(cmd, binary)?;
    host_rules::same_treatment(host, cmd, &wrapped, cwd).then_some(wrapped)
}

/// The wrapped form of `cmd`, or `None` when it must run as written.
pub(super) fn candidate(cmd: &str, binary: &str) -> Option<String> {
    let cmd = cmd.trim();
    if cmd.is_empty()
        || cmd.starts_with("sobre ")
        || cmd.starts_with(&format!("{binary} "))
        || cmd.starts_with(&format!("{} ", crate::hooks::shell_word(binary)))
        || cmd.contains('\n')
        || cmd.contains("<<")
        || cmd.contains("$(")
        || cmd.contains('`')
        || cmd.contains('>')
        || cmd.contains('<')
    {
        return None;
    }
    if compound_lexer::has_control_operator(cmd)
        || crate::rewrite_registry::is_file_read_command(cmd)
        || !crate::rewrite_registry::is_rewritable_command(cmd)
    {
        return None;
    }
    wrap_single_command(cmd, binary)
}

/// `<binary> -c '<cmd>'`. On Windows the host may run it through cmd or
/// PowerShell, which quote differently: only a command and a binary path
/// that need no quoting beyond plain double quotes are wrapped there; any
/// other runs as written.
fn wrap_single_command(cmd: &str, binary: &str) -> Option<String> {
    if cfg!(windows) {
        let needs_care = |s: &str| s.contains(['"', '\'', '`', '$', '%', ' ']);
        if needs_care(binary) || cmd.contains(['"', '\'', '`', '$', '%']) {
            return None;
        }
        Some(format!("{binary} -c \"{cmd}\""))
    } else {
        let escaped = cmd.replace('\'', "'\\''");
        Some(format!(
            "{} -c '{escaped}'",
            crate::hooks::shell_word(binary)
        ))
    }
}

/// The host's "replace the command" reply, with no permission field. Every
/// other argument the model gave (Gemini's `description` and `dir_path`,
/// Cursor's `cwd`) is sent back unchanged, so a host that replaces the input
/// wholesale loses nothing.
fn reply(host: Host, args: Option<&Value>, wrapped: &str) -> String {
    let mut input = args.and_then(Value::as_object).cloned().unwrap_or_default();
    input.insert("command".into(), Value::String(wrapped.to_string()));
    let input = Value::Object(input);
    match host {
        Host::Cursor => json!({ "updated_input": input }).to_string(),
        Host::Gemini => json!({ "hookSpecificOutput": { "tool_input": input } }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIN: &str = "sobre";

    #[test]
    fn single_known_commands_are_wrapped_whole() {
        assert_eq!(
            candidate("git status", BIN).as_deref(),
            Some("sobre -c 'git status'")
        );
        assert_eq!(
            candidate("cargo test --lib", BIN).as_deref(),
            Some("sobre -c 'cargo test --lib'")
        );
    }

    /// The wrapper names a binary whose path holds a space as one word
    /// (on Windows, where cmd and PowerShell quote differently, the command
    /// runs as written).
    #[test]
    fn a_binary_path_with_a_space_stays_one_word() {
        let wrapped = candidate("git status", "/Users/Jane Doe/bin/sobre");
        if cfg!(windows) {
            assert_eq!(wrapped, None);
        } else {
            assert_eq!(
                wrapped.as_deref(),
                Some("'/Users/Jane Doe/bin/sobre' -c 'git status'")
            );
        }
    }

    #[test]
    fn anything_else_runs_as_written() {
        for cmd in [
            "git status && git log",
            "git status & rm -rf target",
            "git log | head",
            "cargo build > out.txt",
            "git commit -m \"$(date)\"",
            "cat <<EOF\nx\nEOF",
            "cat src/main.rs",
            "head -n 5 a.txt",
            "rm -rf target",
            "sobre -c 'git status'",
            "",
        ] {
            assert_eq!(candidate(cmd, BIN), None, "{cmd}");
        }
    }

    /// Two calls of the same command in different directories, close enough
    /// to fall in the double-fire window, each get their own reply: the
    /// second must not replay the first's `cwd`.
    #[cfg(unix)]
    #[test]
    fn the_same_command_in_two_directories_gets_two_replies() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("HOME");
        crate::test_env::set_var("HOME", home.path());

        let reply_for = |dir: &Path| {
            let input = json!({
                "tool_name": "Shell",
                "tool_input": { "command": "git status", "cwd": dir.to_string_lossy() },
                "cwd": dir.to_string_lossy(),
            })
            .to_string();
            let out: Value = serde_json::from_str(&compute_from(Host::Cursor, &input)).unwrap();
            out["updated_input"]["cwd"].as_str().map(str::to_string)
        };
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let first = reply_for(a.path());
        let second = reply_for(b.path());

        match previous_home {
            Some(h) => crate::test_env::set_var("HOME", h),
            None => crate::test_env::remove_var("HOME"),
        }
        assert_eq!(first.as_deref(), Some(&*a.path().to_string_lossy()));
        assert_eq!(second.as_deref(), Some(&*b.path().to_string_lossy()));
    }

    #[test]
    fn replies_carry_no_permission_decision() {
        let args = json!({ "command": "git status", "cwd": "/repo" });
        let cursor = reply(Host::Cursor, Some(&args), "sobre -c 'git status'");
        let v: Value = serde_json::from_str(&cursor).unwrap();
        assert_eq!(v["updated_input"]["command"], "sobre -c 'git status'");
        assert_eq!(v["updated_input"]["cwd"], "/repo");
        assert!(!cursor.contains("permission") && !cursor.contains("allow"));

        let gemini_args = json!({
            "command": "git status",
            "description": "Show status",
            "dir_path": "sub"
        });
        let gemini = reply(Host::Gemini, Some(&gemini_args), "sobre -c 'git status'");
        let v: Value = serde_json::from_str(&gemini).unwrap();
        let input = &v["hookSpecificOutput"]["tool_input"];
        assert_eq!(input["command"], "sobre -c 'git status'");
        assert_eq!(input["description"], "Show status");
        assert_eq!(input["dir_path"], "sub");
        assert!(!gemini.contains("decision"));
    }
}
