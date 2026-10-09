mod claude;
mod codex;
mod copilot;
mod cursor;
mod gemini;
mod opencode;
mod windsurf;

/// Points every sobre hook command containing `marker` in `entry` — a hook,
/// or a group that holds `hooks: [...]` — at `command`, and says whether it
/// found one. A binary that moved leaves its old path in hooks that are only
/// ever added once; this keeps them in step with the binary `init` runs
/// from. Another tool's hooks are left alone.
fn refresh_command(entry: &mut serde_json::Value, marker: &str, command: &str) -> bool {
    let mut found = false;
    if let Some(current) = entry.get_mut("command")
        && current
            .as_str()
            .is_some_and(|c| c.contains(marker) && crate::hooks::is_sobre_hook_command(c))
    {
        *current = serde_json::json!(command);
        found = true;
    }
    if let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
        for hook in hooks {
            found |= refresh_command(hook, marker, command);
        }
    }
    found
}

/// [`refresh_command`] over every entry of a hook array.
fn refresh_commands(entries: &mut [serde_json::Value], marker: &str, command: &str) -> bool {
    let mut found = false;
    for entry in entries {
        found |= refresh_command(entry, marker, command);
    }
    found
}

#[cfg(test)]
pub(super) fn refresh_commands_for_test(
    entries: &mut [serde_json::Value],
    marker: &str,
    command: &str,
) -> bool {
    refresh_commands(entries, marker, command)
}

pub(super) use claude::install_claude_hook_config;
pub use codex::install_codex_hook;
pub(crate) use copilot::copilot_hooks_dir;
pub(super) use copilot::install_copilot_hooks;
pub(super) use cursor::install_cursor_hook_config;
pub(super) use gemini::install_gemini_hook_config;
pub(super) use opencode::install_opencode_plugin;
pub(super) use windsurf::install_windsurf_hooks;
