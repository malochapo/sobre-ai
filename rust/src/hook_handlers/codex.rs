//! Codex CLI hook handler: PreToolUse shell rewrite.
//!
//! Extracted from `hook_handlers::mod` (#660/#966 LOC gate). Codex speaks its
//! own hook JSON dialect (`hookSpecificOutput.additionalContext`,
//! `permissionDecision` without Cursor/Claude's dual-format wrapping), so its
//! handlers stay self-contained here rather than reusing the Cursor/Claude
//! output builders in the parent module.

use super::file_rewrite::rewrite_candidate;
use super::{HOOK_STDIN_TIMEOUT, read_stdin_with_timeout, resolve_binary};

pub(super) fn codex_rewrite_output(rewritten: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {
                "command": rewritten
            }
        }
    })
    .to_string()
}

pub fn handle_codex_pretooluse() {
    let binary = resolve_binary();
    let Some(input) = read_stdin_with_timeout(HOOK_STDIN_TIMEOUT) else {
        print!("{}", codex_allow_output());
        return;
    };

    // #809: use serde_json instead of ad-hoc extract_json_field.
    // The old find('"field":') scanner could mis-parse deeply nested
    // or heavily escaped payloads. serde_json handles all edge cases.
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&input) else {
        print!("{}", codex_allow_output());
        return;
    };

    let tool = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !matches!(tool, "Bash" | "bash") {
        print!("{}", codex_allow_output());
        return;
    }

    // Codex sends command at top level or inside tool_input.
    let cmd = parsed
        .get("command")
        .or_else(|| parsed.get("tool_input").and_then(|ti| ti.get("command")))
        .and_then(|v| v.as_str());
    let Some(cmd) = cmd else {
        print!("{}", codex_allow_output());
        return;
    };

    if let Some(rewritten) = rewrite_candidate(cmd, &binary) {
        print!("{}", codex_rewrite_output(&rewritten));
        return;
    }

    // Commands already routed through lean-ctx (e.g. `lean-ctx -c '...'` or
    // `/opt/homebrew/bin/lean-ctx -c '...'`) must pass through — denying them
    // blocks lean-ctx's own CLI surface (#801).
    if cmd.starts_with("lean-ctx ") || cmd.starts_with(&format!("{binary} ")) {
        print!("{}", codex_allow_output());
        return;
    }

    print!("{}", codex_allow_output());
}

/// Allow-passthrough output for the Codex PreToolUse hook (#809).
/// Codex treats an exit-0 hook with no stdout as an allowed tool call. Its
/// `permissionDecision: "allow"` form is only valid when paired with
/// `updatedInput`; emitting it for an unchanged command makes current Codex
/// report `unsupported permissionDecision:allow` (#1019).
pub(super) fn codex_allow_output() -> String {
    String::new()
}
