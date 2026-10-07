pub mod agents;
mod support;

use agents::{
    install_claude_hook, install_claude_hook_config, install_codex_hook, install_copilot_hook,
    install_cursor_hook_config, install_gemini_hook, install_gemini_hook_config,
    install_opencode_hook, install_opencode_plugin, install_windsurf, install_windsurf_hooks,
};
use support::{ensure_codex_hooks_enabled, install_named_json_server};

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
const REFRESHABLE_HOOK_AGENTS: &[&str] = &[
    "claude", "cursor", "gemini", "codex", "windsurf", "copilot", "opencode",
];

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
            file_contains_lean_ctx(&crate::setup::claude_config_dir(home).join("settings.json"))
        }
        "cursor" => file_contains_lean_ctx(&home.join(".cursor/hooks.json")),
        "gemini" => file_contains_lean_ctx(&home.join(".gemini/settings.json")),
        "codex" => {
            let dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
            file_contains_lean_ctx(&dir.join("hooks.json"))
        }
        "windsurf" => file_contains_lean_ctx(&home.join(".codeium/windsurf/hooks.json")),
        "copilot" => {
            // User-level Copilot hooks live under ~/.copilot/hooks (#381).
            file_contains_lean_ctx(&home.join(".copilot/hooks/hooks.json"))
        }
        "opencode" => home.join(".config/opencode/plugins/sobre.js").exists(),
        _ => false,
    }
}

/// Re-render the hook artifacts for an already-configured agent. Only calls
/// narrow, subprocess-free, global installers (never the full agent setup).
fn refresh_agent_hooks(agent: &str, home: &std::path::Path) {
    match agent {
        "claude" => install_claude_hook_config(home),
        "cursor" => install_cursor_hook_config(home),
        "gemini" => install_gemini_hook_config(home),
        "opencode" => install_opencode_plugin(home, &resolve_binary_path()),
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

/// Binary token for **shell-executed** hook commands (`<binary> hook compress`
/// and the like in Claude/Cursor/Gemini/… hook configs).
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
