use std::fs;
use std::path::{Path, PathBuf};

use super::parsers::{remove_sobre_from_json, remove_sobre_from_toml};
use super::{backup_before_modify, safe_remove, safe_write, shorten};

// ---------------------------------------------------------------------------
// MCP config removal (JSON / YAML / TOML)
// ---------------------------------------------------------------------------

pub(super) fn remove_mcp_configs(home: &Path, dry_run: bool) -> bool {
    let claude_cfg_dir_json = std::env::var("CLAUDE_CONFIG_DIR").ok().map_or_else(
        || PathBuf::from("/nonexistent"),
        |d| PathBuf::from(d).join(".claude.json"),
    );
    let configs: Vec<(&str, PathBuf)> = vec![
        ("Cursor", home.join(".cursor/mcp.json")),
        ("Claude Code (config dir)", claude_cfg_dir_json),
        ("Claude Code (home)", home.join(".claude.json")),
        ("Windsurf", home.join(".codeium/windsurf/mcp_config.json")),
        ("Gemini CLI", home.join(".gemini/settings.json")),
        (
            "Codex CLI",
            crate::core::home::resolve_codex_dir()
                .unwrap_or_else(|| home.join(".codex"))
                .join("config.toml"),
        ),
        (
            "OpenCode",
            crate::core::editor_registry::opencode_config_path(home),
        ),
        ("Copilot CLI", home.join(".copilot/mcp-config.json")),
    ];

    let mut removed = false;

    for (name, path) in &configs {
        if !path.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        if !content.contains("sobre") {
            continue;
        }

        let is_toml = path.extension().and_then(|e| e.to_str()) == Some("toml");

        let cleaned = if is_toml {
            Some(remove_sobre_from_toml(&content))
        } else {
            remove_sobre_from_json(&content)
        };

        if let Some(cleaned) = cleaned {
            backup_before_modify(path, dry_run);
            if let Err(e) = safe_write(path, &cleaned, dry_run) {
                tracing::warn!("Failed to update {} config: {}", name, e);
            } else {
                let verb = if dry_run { "Would update" } else { "✓" };
                println!("  {verb} MCP config removed from {name}");
                removed = true;
            }
        }
    }

    // Zed: uses `context_servers` key — handled by remove_sobre_from_json
    let zed_path = crate::core::editor_registry::zed_settings_path(home);
    if zed_path.exists()
        && let Ok(content) = fs::read_to_string(&zed_path)
        && content.contains("sobre")
    {
        backup_before_modify(&zed_path, dry_run);
        if let Some(cleaned) = remove_sobre_from_json(&content) {
            if let Err(e) = safe_write(&zed_path, &cleaned, dry_run) {
                tracing::warn!("Failed to update Zed config: {e}");
            } else {
                let verb = if dry_run { "Would update" } else { "✓" };
                println!("  {verb} MCP config removed from Zed");
                removed = true;
            }
        }
    }

    let vscode_path = crate::core::editor_registry::vscode_mcp_path();
    let label = "VS Code / Copilot";
    if vscode_path.exists()
        && let Ok(content) = fs::read_to_string(&vscode_path)
        && content.contains("sobre")
        && let Some(cleaned) = remove_sobre_from_json(&content)
    {
        backup_before_modify(&vscode_path, dry_run);
        if let Err(e) = safe_write(&vscode_path, &cleaned, dry_run) {
            tracing::warn!("Failed to update {label} config: {e}");
        } else {
            let verb = if dry_run { "Would update" } else { "✓" };
            println!("  {verb} MCP config removed from {label}");
            removed = true;
        }
    }

    removed
}

// ---------------------------------------------------------------------------
// Skill removal
// ---------------------------------------------------------------------------

/// Delete the `sobre` skill directory from every agent that has one. The
/// directory is sobre's alone, so it goes whole.
pub(super) fn remove_skills(home: &Path, dry_run: bool) -> bool {
    let dirs = crate::setup::skill::installed_dirs(home);
    for dir in &dirs {
        let verb = if dry_run { "Would remove" } else { "✓" };
        if dry_run {
            println!("  {verb} skill {}", shorten(dir, home));
        } else if let Err(e) = fs::remove_dir_all(dir) {
            tracing::warn!("Failed to remove {}: {e}", dir.display());
        } else {
            println!("  {verb} skill {}", shorten(dir, home));
        }
    }
    if dirs.is_empty() {
        println!("  · No skill found");
    }
    !dirs.is_empty()
}

// ---------------------------------------------------------------------------
// Hook files removal
// ---------------------------------------------------------------------------

/// Apply hook cleanup result to a file: write cleaned content, remove if entirely
/// sobre, or leave untouched on parse error / no changes.
fn apply_hook_cleanup(path: &Path, label: &str, content: &str, dry_run: bool) -> bool {
    let verb = if dry_run { "Would" } else { "✓" };
    match remove_sobre_from_hooks_json(content) {
        HookCleanupResult::Cleaned(cleaned) => {
            if let Err(e) = safe_write(path, &cleaned, dry_run) {
                tracing::warn!("Failed to update {label}: {e}");
                return false;
            }
            println!("  {verb} {label} cleaned (user settings preserved)");
            true
        }
        HookCleanupResult::EntirelySobre => {
            if let Err(e) = safe_remove(path, dry_run) {
                tracing::warn!("Failed to remove {label}: {e}");
                return false;
            }
            println!("  {verb} {label} removed");
            true
        }
        HookCleanupResult::Unchanged => false,
        HookCleanupResult::ParseError => {
            tracing::warn!("Could not parse {label}, leaving untouched");
            false
        }
    }
}

pub(super) fn remove_hook_files(home: &Path, dry_run: bool) -> bool {
    // The only hook file sobre writes; every other hook lives in a host
    // config and is removed entry by entry below.
    let hook_files: Vec<PathBuf> = vec![home.join(".config/opencode/plugins/sobre.js")];

    let mut removed = false;
    for path in &hook_files {
        if path.exists() {
            if let Err(e) = safe_remove(path, dry_run) {
                tracing::warn!("Failed to remove hook {}: {e}", path.display());
            } else {
                removed = true;
            }
        }
    }

    if removed {
        let verb = if dry_run { "Would remove" } else { "✓" };
        println!("  {verb} OpenCode plugin removed");
    }

    // Claude Code global settings: surgically remove sobre hook entries
    let claude_settings =
        crate::core::editor_registry::claude_state_dir(home).join("settings.json");
    if claude_settings.exists()
        && let Ok(content) = fs::read_to_string(&claude_settings)
        && content.contains("sobre")
    {
        backup_before_modify(&claude_settings, dry_run);
        removed |= apply_hook_cleanup(
            &claude_settings,
            "Claude Code settings.json",
            &content,
            dry_run,
        );
    }

    // hooks.json: surgically remove sobre entries instead of deleting
    for (label, hj_path) in [
        ("Cursor", home.join(".cursor/hooks.json")),
        (
            "Codex",
            crate::core::home::resolve_codex_dir()
                .unwrap_or_else(|| home.join(".codex"))
                .join("hooks.json"),
        ),
        ("Windsurf", home.join(".codeium/windsurf/hooks.json")),
        ("Copilot (global)", home.join(".copilot/hooks/hooks.json")),
        ("Gemini CLI", home.join(".gemini/settings.json")),
    ] {
        if !hj_path.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&hj_path) else {
            continue;
        };
        if !content.contains("sobre") {
            continue;
        }

        backup_before_modify(&hj_path, dry_run);
        removed |= apply_hook_cleanup(&hj_path, label, &content, dry_run);
    }

    removed
}

/// Result of attempting to remove sobre from a JSON config file.
#[derive(Debug)]
pub(super) enum HookCleanupResult {
    /// No sobre references found; file unchanged.
    Unchanged,
    /// sobre entries removed; cleaned JSON content with remaining settings returned.
    Cleaned(String),
    /// File is entirely sobre-only; safe to delete.
    EntirelySobre,
    /// JSON parse failed; file should NOT be touched.
    ParseError,
}

/// Check if a single string value references sobre.
fn str_is_sobre(s: &str) -> bool {
    s.contains("sobre")
}

/// For flat hook entries: check `command`, `bash`, or any string field for sobre.
fn flat_entry_is_sobre(entry: &serde_json::Value) -> bool {
    let Some(obj) = entry.as_object() else {
        return false;
    };
    for key in ["command", "bash"] {
        if let Some(serde_json::Value::String(s)) = obj.get(key)
            && str_is_sobre(s)
        {
            return true;
        }
    }
    false
}

/// For nested hook entries (`{matcher, hooks: [{command: ...}]}`):
/// Remove only sobre sub-hooks, preserving user hooks in the same group.
/// Returns true if the entry was modified or should be removed entirely.
fn clean_nested_entry(entry: &mut serde_json::Value) -> bool {
    let Some(obj) = entry.as_object_mut() else {
        return false;
    };
    let Some(sub_hooks) = obj.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
        return false;
    };
    let before = sub_hooks.len();
    sub_hooks.retain(|h| !flat_entry_is_sobre(h));
    sub_hooks.len() < before
}

/// Remove sobre hook entries from hooks/settings JSON, preserving other entries.
///
/// Handles multiple formats:
/// - Flat: `{command: "sobre ..."}` (Cursor hooks.json)
/// - Nested: `{matcher: "...", hooks: [{type: "command", command: "sobre ..."}]}` (Claude/Codex)
/// - Copilot: `{bash: "sobre ..."}` (Copilot hooks)
pub(super) fn remove_sobre_from_hooks_json(content: &str) -> HookCleanupResult {
    let Ok(mut parsed) = crate::core::jsonc::parse_jsonc(content) else {
        return HookCleanupResult::ParseError;
    };
    let mut modified = false;

    if let Some(hooks) = parsed.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for entries in hooks.values_mut() {
            if let Some(arr) = entries.as_array_mut() {
                let before = arr.len();

                // First: clean sub-hooks inside nested entries
                for entry in arr.iter_mut() {
                    if clean_nested_entry(entry) {
                        modified = true;
                    }
                }

                // Remove entries that are now empty nested groups
                arr.retain(|entry| {
                    if let Some(sub) = entry.get("hooks").and_then(|h| h.as_array())
                        && sub.is_empty()
                    {
                        return false;
                    }
                    true
                });

                // Then: remove flat entries that are sobre
                arr.retain(|entry| {
                    if entry.get("hooks").is_some() {
                        return true; // nested — already handled above
                    }
                    !flat_entry_is_sobre(entry)
                });

                if arr.len() < before {
                    modified = true;
                }
            }
        }
    }

    if !modified {
        return HookCleanupResult::Unchanged;
    }

    // Remove empty event arrays after cleanup
    if let Some(hooks) = parsed.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    }

    // Check if any meaningful content remains. `version` / `$schema` are
    // format boilerplate written by installers (e.g. Copilot hooks.json
    // `{"hooks": {}, "version": 1}`) — an otherwise-empty file is still
    // entirely sobre-owned and safe to delete (GL #558).
    let has_remaining = parsed.as_object().is_some_and(|obj| {
        obj.iter().any(|(key, val)| match key.as_str() {
            "hooks" => val.as_object().is_some_and(|h| !h.is_empty()),
            "version" | "$schema" => false,
            _ => !val.is_null(),
        })
    });

    let pretty = match serde_json::to_string_pretty(&parsed) {
        Ok(s) => s + "\n",
        Err(_) => return HookCleanupResult::ParseError,
    };

    if has_remaining {
        HookCleanupResult::Cleaned(pretty)
    } else {
        HookCleanupResult::EntirelySobre
    }
}
