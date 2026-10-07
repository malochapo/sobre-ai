use std::fs;
use std::path::{Path, PathBuf};

use super::parsers::{
    remove_lean_ctx_block, remove_lean_ctx_from_json, remove_lean_ctx_from_toml,
    remove_lean_ctx_login_block,
};
use super::{backup_before_modify, safe_remove, safe_write, shorten};

pub(super) fn remove_project_agent_files(dry_run: bool) -> bool {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut removed = false;

    // Project-level MCP/hook JSON files: surgically remove lean-ctx entries
    for (rel, label) in [
        (".vscode/mcp.json", "Project .vscode/mcp.json"),
        (".github/mcp.json", "Project .github/mcp.json"),
        (
            ".github/hooks/hooks.json",
            "Project .github/hooks/hooks.json",
        ),
    ] {
        let path = cwd.join(rel);
        if !path.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if !content.contains("lean-ctx") {
            continue;
        }
        backup_before_modify(&path, dry_run);
        // These files use standard MCP JSON format — try hook cleanup which handles both
        removed |= apply_hook_cleanup(&path, label, &content, dry_run);
    }

    // Project-level .claude/settings.local.json: surgically remove lean-ctx hooks
    let claude_settings = cwd.join(".claude/settings.local.json");
    if claude_settings.exists()
        && let Ok(content) = fs::read_to_string(&claude_settings)
        && content.contains("lean-ctx")
    {
        backup_before_modify(&claude_settings, dry_run);
        removed |= apply_hook_cleanup(
            &claude_settings,
            "Project .claude/settings.local.json",
            &content,
            dry_run,
        );
    }

    removed
}

// ---------------------------------------------------------------------------
// Shell hook removal
// ---------------------------------------------------------------------------

pub(super) fn remove_shell_hook(home: &Path, dry_run: bool) -> bool {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let mut removed = false;

    if dry_run {
        println!("  Would remove shell hook dropin files (.zshenv.d, .bashenv.d)");
    } else {
        remove_legacy_env_hooks(home);
    }

    let rc_files: Vec<PathBuf> = vec![
        home.join(".zshrc"),
        home.join(".bashrc"),
        // Bash login profiles: may carry the "source ~/.bashrc" snippet init_posix adds.
        home.join(".bash_profile"),
        home.join(".bash_login"),
        home.join(".profile"),
        home.join(".config/fish/config.fish"),
        #[cfg(windows)]
        home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1"),
    ];

    for rc in &rc_files {
        if !rc.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(rc) else {
            continue;
        };
        if !content.contains("lean-ctx") {
            continue;
        }

        let mut cleaned = remove_lean_ctx_block(&content);
        cleaned = remove_source_lines(&cleaned);
        cleaned = remove_lean_ctx_login_block(&cleaned);
        if cleaned.trim() != content.trim() {
            let bak = rc.with_extension("lean-ctx.bak");
            if !dry_run {
                let _ = fs::copy(rc, &bak);
            }
            if let Err(e) = safe_write(rc, &cleaned, dry_run) {
                tracing::warn!("Failed to update {}: {}", rc.display(), e);
            } else {
                let short = shorten(rc, home);
                let verb = if dry_run { "Would remove" } else { "✓" };
                println!("  {verb} Shell hook removed from {short}");
                if !dry_run {
                    println!("    Backup: {}", shorten(&bak, home));
                }
                removed = true;
            }
        }
    }

    let hook_files = [
        "shell-hook.zsh",
        "shell-hook.bash",
        "shell-hook.fish",
        "shell-hook.ps1",
        "env.sh",
    ];
    let lc_dir = crate::core::paths::config_dir().unwrap_or_else(|_| home.join(".config/lean-ctx"));
    for f in &hook_files {
        let path = lc_dir.join(f);
        if path.exists() {
            let _ = safe_remove(&path, dry_run);
            let verb = if dry_run { "Would remove" } else { "✓" };
            println!("  {verb} Removed {}", path.display());
            removed = true;
        }
    }

    if !removed && !shell.is_empty() {
        println!("  · No shell hook found");
    }

    removed
}

/// Blocks and drop-ins that older versions wrote into `~/.zshenv`,
/// `~/.bashenv`, `~/.zshrc` and `~/.bashrc` (the non-interactive redirect and
/// the `claude`/`codex`/`gemini` aliases). sobre no longer edits shell files;
/// uninstall still removes what an earlier install left.
fn remove_legacy_env_hooks(home: &Path) {
    const HOOK: (&str, &str) = (
        "# >>> lean-ctx shell hook >>>",
        "# <<< lean-ctx shell hook <<<",
    );
    const ALIASES: (&str, &str) = (
        "# >>> lean-ctx agent aliases >>>",
        "# <<< lean-ctx agent aliases <<<",
    );
    let slots: [(&str, &str, &str, (&str, &str)); 4] = [
        (".zshenv", ".zshenv.d", "00-lean-ctx.zsh", HOOK),
        (".bashenv", ".bashenv.d", "00-lean-ctx.sh", HOOK),
        (".zshrc", ".zshrc.d", "00-lean-ctx.zsh", ALIASES),
        (".bashrc", ".bashrc.d", "00-lean-ctx.sh", ALIASES),
    ];
    for (rc, dropin_dir, dropin_file, (start, end)) in slots {
        let label = format!("legacy lean-ctx block in ~/{rc}");
        crate::marked_block::remove_from_file(&home.join(rc), start, end, false, &label);
        let dropin = home.join(dropin_dir).join(dropin_file);
        if dropin.exists() {
            match fs::remove_file(&dropin) {
                Ok(()) => println!("  Removed {label} from {}", dropin.display()),
                Err(e) => tracing::error!("Cannot remove {}: {e}", dropin.display()),
            }
        }
    }
}

fn remove_source_lines(content: &str) -> String {
    content
        .lines()
        .filter(|line| {
            !line.contains("lean-ctx/shell-hook.") && !line.contains("lean-ctx\\shell-hook.")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

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
        ("OpenCode", home.join(".config/opencode/opencode.json")),
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
        if !content.contains("lean-ctx") {
            continue;
        }

        let is_toml = path.extension().and_then(|e| e.to_str()) == Some("toml");

        let cleaned = if is_toml {
            Some(remove_lean_ctx_from_toml(&content))
        } else {
            remove_lean_ctx_from_json(&content)
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

    // Zed: uses `context_servers` key — handled by remove_lean_ctx_from_json
    let zed_path = crate::core::editor_registry::zed_settings_path(home);
    if zed_path.exists()
        && let Ok(content) = fs::read_to_string(&zed_path)
        && content.contains("lean-ctx")
    {
        backup_before_modify(&zed_path, dry_run);
        if let Some(cleaned) = remove_lean_ctx_from_json(&content) {
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
        && content.contains("lean-ctx")
        && let Some(cleaned) = remove_lean_ctx_from_json(&content)
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
/// lean-ctx, or leave untouched on parse error / no changes.
fn apply_hook_cleanup(path: &Path, label: &str, content: &str, dry_run: bool) -> bool {
    let verb = if dry_run { "Would" } else { "✓" };
    match remove_lean_ctx_from_hooks_json(content) {
        HookCleanupResult::Cleaned(cleaned) => {
            if let Err(e) = safe_write(path, &cleaned, dry_run) {
                tracing::warn!("Failed to update {label}: {e}");
                return false;
            }
            println!("  {verb} {label} cleaned (user settings preserved)");
            true
        }
        HookCleanupResult::EntirelyLeanCtx => {
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

    // Claude Code global settings: surgically remove lean-ctx hook entries
    // Both settings.json and settings.local.json can contain hooks
    for claude_settings_name in ["settings.json", "settings.local.json"] {
        let claude_settings =
            crate::core::editor_registry::claude_state_dir(home).join(claude_settings_name);
        if !claude_settings.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&claude_settings) else {
            continue;
        };
        if !content.contains("lean-ctx") {
            continue;
        }
        backup_before_modify(&claude_settings, dry_run);
        removed |= apply_hook_cleanup(
            &claude_settings,
            &format!("Claude Code {claude_settings_name}"),
            &content,
            dry_run,
        );
    }

    // hooks.json: surgically remove lean-ctx entries instead of deleting
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
        if !content.contains("lean-ctx") {
            continue;
        }

        backup_before_modify(&hj_path, dry_run);
        removed |= apply_hook_cleanup(&hj_path, label, &content, dry_run);
    }

    removed
}

/// Result of attempting to remove lean-ctx from a JSON config file.
#[derive(Debug)]
pub(super) enum HookCleanupResult {
    /// No lean-ctx references found; file unchanged.
    Unchanged,
    /// lean-ctx entries removed; cleaned JSON content with remaining settings returned.
    Cleaned(String),
    /// File is entirely lean-ctx-only; safe to delete.
    EntirelyLeanCtx,
    /// JSON parse failed; file should NOT be touched.
    ParseError,
}

/// Check if a single string value references lean-ctx.
fn str_is_lean_ctx(s: &str) -> bool {
    s.contains("lean-ctx")
}

/// For flat hook entries: check `command`, `bash`, or any string field for lean-ctx.
fn flat_entry_is_lean_ctx(entry: &serde_json::Value) -> bool {
    let Some(obj) = entry.as_object() else {
        return false;
    };
    for key in ["command", "bash"] {
        if let Some(serde_json::Value::String(s)) = obj.get(key)
            && str_is_lean_ctx(s)
        {
            return true;
        }
    }
    false
}

/// For nested hook entries (`{matcher, hooks: [{command: ...}]}`):
/// Remove only lean-ctx sub-hooks, preserving user hooks in the same group.
/// Returns true if the entry was modified or should be removed entirely.
fn clean_nested_entry(entry: &mut serde_json::Value) -> bool {
    let Some(obj) = entry.as_object_mut() else {
        return false;
    };
    let Some(sub_hooks) = obj.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
        return false;
    };
    let before = sub_hooks.len();
    sub_hooks.retain(|h| !flat_entry_is_lean_ctx(h));
    sub_hooks.len() < before
}

/// Remove lean-ctx hook entries from hooks/settings JSON, preserving other entries.
///
/// Handles multiple formats:
/// - Flat: `{command: "lean-ctx ..."}` (Cursor hooks.json)
/// - Nested: `{matcher: "...", hooks: [{type: "command", command: "lean-ctx ..."}]}` (Claude/Codex)
/// - Copilot: `{bash: "lean-ctx ..."}` (Copilot hooks)
pub(super) fn remove_lean_ctx_from_hooks_json(content: &str) -> HookCleanupResult {
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

                // Then: remove flat entries that are lean-ctx
                arr.retain(|entry| {
                    if entry.get("hooks").is_some() {
                        return true; // nested — already handled above
                    }
                    !flat_entry_is_lean_ctx(entry)
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

    if parsed
        .get("permissions")
        .and_then(|p| p.as_object())
        .is_some_and(serde_json::Map::is_empty)
    {
        parsed.as_object_mut().map(|o| o.remove("permissions"));
    }

    // Check if any meaningful content remains. `version` / `$schema` are
    // format boilerplate written by installers (e.g. Copilot hooks.json
    // `{"hooks": {}, "version": 1}`) — an otherwise-empty file is still
    // entirely lean-ctx-owned and safe to delete (GL #558).
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
        HookCleanupResult::EntirelyLeanCtx
    }
}
