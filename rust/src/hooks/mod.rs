use std::path::PathBuf;

pub mod agents;
mod support;

use agents::{
    install_claude_hook, install_claude_hook_config, install_claude_hook_scripts,
    install_codex_hook, install_copilot_hook, install_cursor_hook_config,
    install_cursor_hook_scripts, install_gemini_hook, install_gemini_hook_config,
    install_gemini_hook_scripts, install_opencode_hook, install_windsurf, install_windsurf_hooks,
};
use support::{
    ensure_codex_hooks_enabled, install_named_json_server, upsert_lean_ctx_codex_hook_entries,
};

fn mcp_server_quiet_mode() -> bool {
    crate::core::runtime_flags::mcp_server_enabled() || crate::core::runtime_flags::quiet_enabled()
}

/// Agents whose global shell-hook artifacts embed the binary path / command
/// and therefore must be re-rendered after an update or on MCP server start so
/// they always point at the current binary. Each entry is gated on a detection
/// marker (see `hooks_installed_for`) so we never install hooks for an agent
/// the user never configured. The `refresh_covers_every_hybrid_agent` test
/// proves this list plus `REFRESH_EXEMPT_AGENTS` accounts for every hook-capable
/// agent, so a newly added agent can never silently regress.
const REFRESHABLE_HOOK_AGENTS: &[&str] =
    &["claude", "cursor", "gemini", "codex", "windsurf", "copilot"];

/// Hook-capable agents intentionally NOT auto-refreshed, with the reason each is safe
/// to skip. Refresh runs silently (including on every MCP server start), so it
/// must never spawn subprocesses or write project/cwd-relative files. Used by
/// the coverage test to prove every hook-capable agent has an explicit decision.
#[cfg(test)]
const HOOK_CAPABLE_AGENTS: &[&str] = &[
    "cursor",
    "gemini",
    "codex",
    "claude",
    "claude-code",
    "opencode",
    "windsurf",
    "copilot",
];

#[cfg(test)]
const REFRESH_EXEMPT_AGENTS: &[&str] = &[
    // Alias of `claude` — same global files, already refreshed via "claude".
    "claude-code",
    // MCP-config wiring only (no global binary-embedding shell-hook script to
    // keep current); refreshed by `setup --fix`, not on start.
    "opencode",
];

/// Silently refresh all hook scripts for agents that are already configured.
/// Called after updates and on MCP server start to ensure hooks match the
/// current binary version. Every hook-capable agent with a global shell hook is
/// covered (the rest are explicitly exempted, enforced by test).
pub fn refresh_installed_hooks() {
    let Some(home) = crate::core::home::resolve_home_dir() else {
        return;
    };
    for agent in REFRESHABLE_HOOK_AGENTS {
        if hooks_installed_for(agent, &home) {
            refresh_agent_hooks(agent, &home);
        }
    }
}

/// True when `agent` already has lean-ctx hook artifacts on disk (global only).
fn hooks_installed_for(agent: &str, home: &std::path::Path) -> bool {
    match agent {
        "claude" => {
            let dir = crate::setup::claude_config_dir(home);
            dir.join("hooks/lean-ctx-rewrite.sh").exists()
                || file_contains_lean_ctx(&dir.join("settings.json"))
        }
        "cursor" => {
            home.join(".cursor/hooks/lean-ctx-rewrite.sh").exists()
                || file_contains_lean_ctx(&home.join(".cursor/hooks.json"))
        }
        "gemini" => {
            home.join(".gemini/hooks/lean-ctx-rewrite-gemini.sh")
                .exists()
                || home.join(".gemini/hooks/lean-ctx-hook-gemini.sh").exists()
        }
        "codex" => {
            let dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
            dir.join("hooks/lean-ctx-rewrite-codex.sh").exists()
                || file_contains_lean_ctx(&dir.join("hooks.json"))
        }
        "windsurf" => file_contains_lean_ctx(&home.join(".codeium/windsurf/hooks.json")),
        "copilot" => {
            // User-level Copilot hooks live under ~/.copilot/hooks (#381).
            file_contains_lean_ctx(&home.join(".copilot/hooks/hooks.json"))
        }
        _ => false,
    }
}

/// Re-render the hook artifacts for an already-configured agent. Only calls
/// narrow, subprocess-free, global installers (never the full agent setup).
fn refresh_agent_hooks(agent: &str, home: &std::path::Path) {
    match agent {
        "claude" => {
            install_claude_hook_scripts(home);
            install_claude_hook_config(home);
        }
        "cursor" => {
            install_cursor_hook_scripts(home);
            install_cursor_hook_config(home);
        }
        "gemini" => {
            install_gemini_hook_scripts(home);
            install_gemini_hook_config(home);
        }
        "codex" => install_codex_hook(),
        "windsurf" => install_windsurf_hooks(home),
        "copilot" => install_copilot_hook(),
        _ => {}
    }
}

fn file_contains_lean_ctx(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|c| c.contains("lean-ctx"))
}

/// Resolve the lean-ctx binary to an **absolute** path for generated hook
/// commands and MCP server entries.
///
/// Agent hooks (Codex, Cursor, Claude, Gemini, …) are executed by
/// the host under a plain non-login shell (`sh -c …`) whose `PATH` is not
/// guaranteed to contain the install dir (e.g. `/usr/local/bin`). A bare
/// `lean-ctx` therefore fails with exit code 127 (#367). Always emitting the
/// resolved absolute path makes hook execution deterministic and matches what
/// MCP setup (`setup/mcp.rs`) and `doctor` already do. Existing configs with a
/// bare command are rewritten on the next `lean-ctx init` / `doctor` run.
///
/// Kept strictly absolute — also used for MCP server `command` fields, which
/// hosts spawn **directly** (no shell), so `$HOME/...` forms would break
/// there. Shell-executed hook commands go through
/// [`resolve_hook_command_binary`], which honors the portable override (#708).
fn resolve_binary_path() -> String {
    crate::core::portable_binary::resolve_portable_binary()
}

/// Binary token for **shell-executed** hook commands (`<binary> hook rewrite`
/// in Claude/Cursor/Gemini/… hook configs and generated `#!/bin/sh` scripts).
///
/// Portable override (#708): the
/// `hook_binary`, is emitted **verbatim** — for settings files synced across
/// machines with different usernames (`$HOME/.local/bin/lean-ctx`). Hook
/// hosts run these commands through a shell, so the variable expands at
/// execution time; `doctor` accepts the override as current, so
/// `init`/`--fix`/`update` stop rewriting synced files. MCP registrations
/// and autostart units keep the absolute path (no shell there).
fn resolve_hook_command_binary() -> String {
    if let Some(portable) = crate::core::portable_binary::hook_binary_override() {
        return portable;
    }
    resolve_binary_path()
}

fn resolve_binary_path_for_bash() -> String {
    if let Some(portable) = crate::core::portable_binary::hook_binary_override() {
        return portable;
    }
    to_bash_compatible_path(&resolve_binary_path())
}

/// Shell-quotes a binary token for generated `#!/bin/sh` wrappers and
/// `LEAN_CTX_BIN=` assignments (#719). Double quotes keep `$HOME`-style
/// portable overrides (#708) expanding at execution time while paths with
/// spaces survive word splitting (npm installs under
/// `C:\Users\First Last\AppData\…`). `"` and `` ` `` are escaped; `$` stays
/// active on purpose — portable forms rely on it.
pub(crate) fn shell_quoted_binary(binary: &str) -> String {
    let escaped = binary.replace('"', "\\\"").replace('`', "\\`");
    format!("\"{escaped}\"")
}

/// #719: true when an existing generated wrapper references a portable binary
/// form (`$HOME/…`, `${HOME}/…`, `%USERPROFILE%\…`) that resolves to an
/// existing binary on THIS machine. Such a wrapper is healthy and must not be
/// re-stamped with a machine-absolute path: on multi-machine synced setups
/// (Dropbox'd `~/.claude`, different usernames) a heal on the machine WITHOUT
/// the portable override would otherwise bake its absolute path into the
/// wrapper, and every new session on the peer machine dies mid tool call with
/// no surfaced error.
fn wrapper_is_portable_and_working(path: &std::path::Path, home: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|content| wrapper_content_is_portable_and_working(&content, home))
}

pub(crate) fn wrapper_content_is_portable_and_working(
    content: &str,
    home: &std::path::Path,
) -> bool {
    let Some(token) = wrapper_binary_token(content) else {
        return false;
    };
    if !(token.contains("$HOME") || token.contains("${HOME}") || token.contains("%USERPROFILE%")) {
        return false;
    }
    let home_s = home.to_string_lossy();
    let expanded = token
        .replace("${HOME}", &home_s)
        .replace("$HOME", &home_s)
        .replace("%USERPROFILE%", &home_s);
    std::path::Path::new(&from_bash_to_native_path(&expanded)).exists()
}

/// Extracts the binary token from a generated wrapper script: the
/// `LEAN_CTX_BIN=` assignment (rewrite scripts) or the `exec <binary> hook …`
/// line (native wrappers) — quoted or bare.
pub(crate) fn wrapper_binary_token(content: &str) -> Option<String> {
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("LEAN_CTX_BIN=") {
            let rest = rest.trim();
            let tok = rest
                .strip_prefix('"')
                .map_or(rest, |r| r.split('"').next().unwrap_or_default());
            if !tok.is_empty() {
                return Some(tok.to_string());
            }
        }
        if let Some(rest) = t.strip_prefix("exec ") {
            let rest = rest.trim();
            let tok = match rest.strip_prefix('"') {
                Some(r) => r.split('"').next().unwrap_or_default().to_string(),
                None => rest
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            };
            if !tok.is_empty() {
                return Some(tok);
            }
        }
    }
    None
}

/// Writes a generated hook wrapper unless the portable override is unset AND
/// the existing file already carries a working portable reference (#719) —
/// healing must never replace a synced portable wrapper with a
/// machine-absolute path.
fn write_wrapper_file(path: &std::path::Path, content: &str, home: &std::path::Path) {
    if crate::core::portable_binary::hook_binary_override().is_none()
        && wrapper_is_portable_and_working(path, home)
    {
        return;
    }
    write_file(path, content);
}

pub fn to_bash_compatible_path(path: &str) -> String {
    let path = match crate::core::pathutil::strip_verbatim_str(path) {
        Some(stripped) => stripped,
        None => path.replace('\\', "/"),
    };
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        let drive = (path.as_bytes()[0] as char).to_ascii_lowercase();
        format!("/{drive}{}", &path[2..])
    } else {
        path
    }
}

/// Convert a Unix/MSYS-style path (`/c/Users/...`) back to native Windows
/// format (`C:/Users/...`). No-op for paths that don't match the pattern.
pub fn from_bash_to_native_path(path: &str) -> String {
    crate::core::pathutil::normalize_tool_path(path)
}

pub fn generate_rewrite_script(binary: &str) -> String {
    let case_pattern = crate::rewrite_registry::bash_case_pattern();
    // #719: assignment + rewritten command carry the binary quoted, so
    // portable `$HOME/…` overrides expand at exec time and paths with spaces
    // survive word splitting.
    let quoted_binary = shell_quoted_binary(binary);
    format!(
        r#"#!/usr/bin/env bash
# lean-ctx PreToolUse hook — rewrites bash commands to lean-ctx equivalents
set -euo pipefail

LEAN_CTX_BIN={quoted_binary}

INPUT=$(cat)
TOOL=$(echo "$INPUT" | grep -oE '"tool_name":"([^"\\]|\\.)*"' | head -1 | sed 's/^"tool_name":"//;s/"$//' | sed 's/\\"/"/g;s/\\\\/\\/g')

case "$TOOL" in
  Bash|bash|PowerShell|powershell) ;;
  *) exit 0 ;;
esac

CMD=$(echo "$INPUT" | grep -oE '"command":"([^"\\]|\\.)*"' | head -1 | sed 's/^"command":"//;s/"$//' | sed 's/\\"/"/g;s/\\\\/\\/g')

if [ -z "$CMD" ] || echo "$CMD" | grep -qE "^(lean-ctx |\"?$LEAN_CTX_BIN\"? )"; then
  exit 0
fi

# Skip multi-line commands: the grep/sed extraction above does not decode
# JSON \n into real newlines, so lean-ctx -c would receive fused lines (#787).
if printf '%s' "$CMD" | grep -qF '\n'; then exit 0; fi

case "$CMD" in
  {case_pattern})
    # Shell-escape then JSON-escape (two passes)
    SHELL_ESC=$(printf '%s' "$CMD" | sed 's/\\/\\\\/g;s/"/\\"/g')
    REWRITE="\"$LEAN_CTX_BIN\" -c \"$SHELL_ESC\""
    JSON_CMD=$(printf '%s' "$REWRITE" | sed 's/\\/\\\\/g;s/"/\\"/g')
    printf '{{"hookSpecificOutput":{{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{{"command":"%s"}}}}}}' "$JSON_CMD"
    ;;
  *) exit 0 ;;
esac
"#
    )
}

pub fn generate_compact_rewrite_script(binary: &str) -> String {
    let case_pattern = crate::rewrite_registry::bash_case_pattern();
    let quoted_binary = shell_quoted_binary(binary);
    format!(
        r#"#!/usr/bin/env bash
# lean-ctx hook — rewrites shell commands
set -euo pipefail
LEAN_CTX_BIN={quoted_binary}
INPUT=$(cat)
CMD=$(echo "$INPUT" | grep -oE '"command":"([^"\\]|\\.)*"' | head -1 | sed 's/^"command":"//;s/"$//' | sed 's/\\"/"/g;s/\\\\/\\/g' 2>/dev/null || echo "")
if [ -z "$CMD" ] || echo "$CMD" | grep -qE "^(lean-ctx |\"?$LEAN_CTX_BIN\"? )"; then exit 0; fi
if printf '%s' "$CMD" | grep -qF '\n'; then exit 0; fi
case "$CMD" in
  {case_pattern})
    SHELL_ESC=$(printf '%s' "$CMD" | sed 's/\\/\\\\/g;s/"/\\"/g')
    REWRITE="\"$LEAN_CTX_BIN\" -c \"$SHELL_ESC\""
    JSON_CMD=$(printf '%s' "$REWRITE" | sed 's/\\/\\\\/g;s/"/\\"/g')
    printf '{{"hookSpecificOutput":{{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{{"command":"%s"}}}}}}' "$JSON_CMD" ;;
  *) exit 0 ;;
esac
"#
    )
}

const REDIRECT_SCRIPT_CLAUDE: &str = r"#!/usr/bin/env bash
# lean-ctx PreToolUse hook — all native tools pass through
# Read/Grep/ListFiles are allowed so Edit (which requires native Read) works.
# The MCP instructions guide the AI to prefer ctx_read/ctx_search/ctx_tree.
exit 0
";

const REDIRECT_SCRIPT_GENERIC: &str = r"#!/usr/bin/env bash
# lean-ctx hook — all native tools pass through
exit 0
";

/// Install everything sobre puts into an agent's own configuration: the MCP
/// server entry, the `sobre` skill where the agent supports skills and, where
/// the host supports them, the shell/read hooks. The same for every agent —
/// there is no mode to choose.
pub fn install_agent_hook(agent: &str) {
    let home = crate::core::home::resolve_home_dir().unwrap_or_default();
    match crate::setup::skill::install(agent, &home) {
        Ok(Some(path)) if !mcp_server_quiet_mode() => {
            eprintln!("Installed the sobre skill at {}", path.display());
        }
        Err(e) => eprintln!("Could not install the sobre skill: {e}"),
        _ => {}
    }
    match agent {
        "claude" | "claude-code" => install_claude_hook(),
        "cursor" => agents::install_cursor_hook(),
        "gemini" => install_gemini_hook(),
        "codex" => install_codex_hook(),
        "windsurf" => install_windsurf(),
        "copilot" | "vscode" => install_copilot_hook(),
        "opencode" => install_opencode_hook(),
        "zed" => {
            let zed_path = crate::core::editor_registry::zed_settings_path(&home);
            let binary = resolve_binary_path();
            let entry = full_server_entry(&binary);
            install_named_json_server("Zed", "settings.json", &zed_path, "context_servers", entry);
        }
        _ => {
            eprintln!("Unknown agent: {agent}");
            eprintln!(
                "  Supported: claude, codex, copilot, cursor, gemini, opencode, vscode, windsurf, zed"
            );
            std::process::exit(1);
        }
    }
}

fn write_file(path: &std::path::Path, content: &str) {
    // Skip identical rewrites: re-running setup/init must not churn mtimes or
    // leave .bak files behind for content that did not change (GL #558).
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == content) {
        return;
    }
    if let Err(e) = crate::config_io::write_atomic_with_backup(path, content) {
        tracing::error!("Error writing {}: {e}", path.display());
    }
}

/// Create a setup directory, surfacing a clear error instead of silently
/// swallowing it (#596).
///
/// A user may symlink `~/.claude` / `~/.codex` (or a child) into a dotfiles
/// repo; [`crate::config_io::ensure_dir`] follows such a symlink to its real
/// in-`$HOME` target and tolerates a dangling one. Returns `false` (after
/// printing the reason) when the directory cannot be prepared, so the caller can
/// skip the now-impossible writes rather than failing confusingly downstream.
fn ensure_state_dir(dir: &std::path::Path) -> bool {
    match crate::config_io::ensure_dir(dir) {
        Ok(()) => true,
        Err(e) => {
            // Always surface — a swallowed dir failure was the #596 footgun.
            eprintln!("lean-ctx init: cannot prepare {}: {e}", dir.display());
            false
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_path: &PathBuf) {}

/// The MCP server entry for the JSON-config agents. It carries no `env`: the
/// server resolves its directories and project root itself.
fn full_server_entry(binary: &str) -> serde_json::Value {
    serde_json::json!({ "command": binary })
}

pub(crate) fn install_mcp_json_agent(
    name: &str,
    display_path: &str,
    config_path: &std::path::Path,
) {
    let binary = resolve_binary_path();
    let entry = full_server_entry(&binary);
    install_named_json_server(name, display_path, config_path, "mcpServers", entry);
}

#[cfg(test)]
mod tests;
