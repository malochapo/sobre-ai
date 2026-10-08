pub mod agents;
mod support;

use agents::{
    install_claude_hook_config, install_codex_hook, install_copilot_hooks,
    install_cursor_hook_config, install_gemini_hook_config, install_opencode_plugin,
    install_windsurf_hooks,
};
pub(crate) use support::{CodexHooksFlag, ensure_codex_hooks_enabled, remove_codex_hooks_flag};

/// Resolve the sobre binary to an **absolute** path for generated hook
/// commands and MCP server entries.
///
/// Agent hooks (Codex, Cursor, Claude, Gemini, …) are executed by
/// the host under a plain non-login shell (`sh -c …`) whose `PATH` is not
/// guaranteed to contain the install dir (e.g. `/usr/local/bin`). A bare
/// `sobre` therefore fails with exit code 127 (#367). Always emitting the
/// resolved absolute path makes hook execution deterministic and matches what
/// MCP setup (`setup/mcp.rs`) and `doctor` already do.
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
/// machines with different usernames (`$HOME/.local/bin/sobre`). Hook
/// hosts run these commands through a shell, so the variable expands at
/// execution time; `doctor` accepts the override as current, so
/// `init`/`--fix` stop rewriting synced files. MCP registrations
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

/// Installs what sobre adds to an agent besides its MCP entry: the `sobre`
/// skill where the agent loads skills, and the hooks where the host has them.
/// The MCP entry itself is written by `editor_registry::write_config`.
/// `Ok(Some(note))` when the install worked but something will keep the hooks
/// from running.
pub fn install_agent_hook(agent: &str) -> Result<Option<String>, String> {
    let home = crate::core::home::resolve_home_dir()
        .ok_or_else(|| "cannot resolve the home directory".to_string())?;
    crate::setup::skill::install(agent, &home)?;
    match agent {
        "claude" | "claude-code" => install_claude_hook_config(&home).map(|()| None),
        "cursor" => install_cursor_hook_config(&home).map(|()| None),
        "gemini" => install_gemini_hook_config(&home).map(|()| None),
        "codex" => install_codex_hook(),
        "windsurf" => install_windsurf_hooks(&home).map(|()| None),
        "copilot" | "vscode" => install_copilot_hooks().map(|()| None),
        "opencode" => install_opencode_plugin(&home, &resolve_binary_path()).map(|()| None),
        "zed" => Ok(None),
        _ => Err(format!(
            "unknown agent '{agent}' (supported: claude, codex, copilot, cursor, gemini, opencode, vscode, windsurf, zed)"
        )),
    }
}

#[cfg(test)]
mod tests;
