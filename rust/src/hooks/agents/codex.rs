use super::super::{
    ensure_codex_hooks_enabled as shared_ensure_codex_hooks_enabled, ensure_state_dir,
    mcp_server_quiet_mode, resolve_hook_command_binary, upsert_lean_ctx_codex_hook_entries,
    write_file,
};

pub fn install_codex_hook() {
    let Some(codex_dir) = crate::core::home::resolve_codex_dir() else {
        tracing::error!("Cannot resolve codex directory");
        return;
    };
    if !ensure_state_dir(&codex_dir) {
        return;
    }

    let hook_config_changed = install_codex_hook_config(&codex_dir);

    if hook_config_changed && !mcp_server_quiet_mode() {
        eprintln!("Installed Codex PreToolUse hook at {}", codex_dir.display());
    }
}

fn install_codex_hook_config(codex_dir: &std::path::Path) -> bool {
    let binary = resolve_hook_command_binary();
    let pre_tool_use_cmd = format!("{binary} hook codex-pretooluse");
    let hooks_json_path = codex_dir.join("hooks.json");

    let mut changed = false;
    let mut root = if hooks_json_path.exists() {
        if let Some(parsed) = std::fs::read_to_string(&hooks_json_path)
            .ok()
            .and_then(|content| crate::core::jsonc::parse_jsonc(&content).ok())
        {
            parsed
        } else {
            changed = true;
            serde_json::json!({ "hooks": {} })
        }
    } else {
        changed = true;
        serde_json::json!({ "hooks": {} })
    };

    if upsert_lean_ctx_codex_hook_entries(&mut root, &pre_tool_use_cmd) {
        changed = true;
    }

    // Observe hooks for context awareness
    let observe_cmd = format!("{binary} hook observe");
    if ensure_codex_observe_hooks(&mut root, &observe_cmd) {
        changed = true;
    }

    if changed {
        write_file(
            &hooks_json_path,
            &serde_json::to_string_pretty(&root).unwrap_or_default(),
        );
    }

    let config_toml_path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| codex_dir.join("config.toml"));
    let config_content = std::fs::read_to_string(&config_toml_path).unwrap_or_default();

    // Hybrid mode: ensure MCP server entry exists in config.toml so Codex
    // Desktop/Cloud can reach lean-ctx even without CLI hooks.
    let mcp_updated = ensure_codex_mcp_server(&config_content, &binary);
    let hooks_updated =
        ensure_codex_hooks_enabled(mcp_updated.as_deref().unwrap_or(&config_content));

    let final_content = hooks_updated
        .or(mcp_updated)
        .unwrap_or_else(|| config_content.clone());
    if final_content != config_content {
        write_file(&config_toml_path, &final_content);
        changed = true;
        if !mcp_server_quiet_mode() {
            eprintln!(
                "Updated Codex config (MCP server + hooks) in {}",
                config_toml_path.display()
            );
        }
    }

    changed
}

fn ensure_codex_observe_hooks(root: &mut serde_json::Value, observe_cmd: &str) -> bool {
    let original = root.clone();
    let Some(hooks_obj) = root
        .as_object_mut()
        .and_then(|r| r.get_mut("hooks"))
        .and_then(|h| h.as_object_mut())
    else {
        return false;
    };

    let observe_events = ["PostToolUse", "SessionStart", "SessionEnd"];
    for event in observe_events {
        let arr = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| serde_json::json!([]));
        let Some(entries) = arr.as_array_mut() else {
            continue;
        };
        let already = entries.iter().any(|e| {
            e.get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("hook observe"))
                    })
                })
        });
        if !already {
            entries.push(serde_json::json!({
                "matcher": ".*",
                "hooks": [{ "type": "command", "command": observe_cmd, "timeout": 5 }]
            }));
        }
    }

    *root != original
}

/// Ensure `[mcp_servers.lean-ctx]` exists in Codex's `config.toml`.
///
/// Existing `command`/`args` are left untouched to respect user customization.
/// The entry carries no `env` table: older installs pinned
/// `LEAN_CTX_PROJECT_ROOT` / `LEAN_CTX_DATA_DIR` there, and the server reads
/// neither from its environment any more, so a stale table — including an
/// orphaned `[mcp_servers.lean-ctx.env]` without its parent (issue #189) — is
/// dropped. Returns `None` when nothing changed, or when the file is not valid
/// TOML (never clobbers an unparseable user config).
fn ensure_codex_mcp_server(config_content: &str, binary: &str) -> Option<String> {
    let mut doc = config_content.parse::<toml_edit::DocumentMut>().ok()?;
    let original = doc.to_string();

    // `[mcp_servers]` stays implicit so we never emit a bare parent header.
    let servers = doc["mcp_servers"].or_insert(toml_edit::table());
    if let Some(t) = servers.as_table_mut() {
        t.set_implicit(true);
    }

    // `[mcp_servers.lean-ctx]` must be explicit so its header is rendered before
    // the `.env` child table (fixes the orphaned-env ordering from #189).
    let lean = servers["lean-ctx"].or_insert(toml_edit::table());
    let lean_tbl = lean.as_table_mut()?;
    lean_tbl.set_implicit(false);

    // Respect user customization: only fill `command`/`args` when absent.
    if !lean_tbl.contains_key("command") {
        lean_tbl["command"] = toml_edit::value(binary);
    }
    if !lean_tbl.contains_key("args") {
        lean_tbl["args"] = toml_edit::value(toml_edit::Array::new());
    }

    lean_tbl.remove("env");

    let updated = doc.to_string();
    (updated != original).then_some(updated)
}

fn ensure_codex_hooks_enabled(config_content: &str) -> Option<String> {
    shared_ensure_codex_hooks_enabled(config_content)
}

// The codex-pretooluse handler rewrites compressible Bash calls and lets every
// other call through unchanged (Codex PreToolUse only fires for Bash).

#[cfg(test)]
mod tests {
    use super::{
        ensure_codex_hooks_enabled, ensure_codex_mcp_server, upsert_lean_ctx_codex_hook_entries,
    };
    use serde_json::json;

    #[test]
    fn upsert_replaces_lean_ctx_hook_but_keeps_custom_hooks() {
        let mut input = json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/opt/homebrew/bin/lean-ctx hook codex-pretooluse",
                            "timeout": 15
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "echo keep-me",
                            "timeout": 5
                        }]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "echo keep-post",
                            "timeout": 5
                        }]
                    }
                ]
            }
        });

        let changed =
            upsert_lean_ctx_codex_hook_entries(&mut input, "lean-ctx hook codex-pretooluse");
        assert!(changed, "the stale binary path should be rewritten");

        let pre_tool_use = input["hooks"]["PreToolUse"]
            .as_array()
            .expect("PreToolUse array should remain");
        assert_eq!(pre_tool_use.len(), 2, "custom hook should be preserved");
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str(),
            Some("echo keep-me")
        );
        assert_eq!(
            pre_tool_use[1]["hooks"][0]["command"].as_str(),
            Some("lean-ctx hook codex-pretooluse")
        );
        assert_eq!(
            input["hooks"]["PostToolUse"][0]["hooks"][0]["command"].as_str(),
            Some("echo keep-post")
        );
    }

    #[test]
    fn ignores_non_lean_ctx_codex_entries() {
        let custom = json!({
            "matcher": "Bash",
            "hooks": [{
                "type": "command",
                "command": "echo keep-me",
                "timeout": 5
            }]
        });
        assert!(
            !crate::hooks::support::is_lean_ctx_codex_managed_entry("PreToolUse", &custom),
            "custom Codex hooks must be preserved"
        );
    }

    #[test]
    fn ensure_codex_hooks_enabled_updates_existing_features_flag() {
        let input = "\
[features]
other = true
codex_hooks = false

[mcp_servers.other]
command = \"other\"
";

        let output =
            ensure_codex_hooks_enabled(input).expect("codex_hooks=false should be migrated");

        assert!(output.contains("[features]\nother = true\nhooks = true\n"));
        assert!(!output.contains("codex_hooks = false"));
    }

    #[test]
    fn ensure_codex_hooks_enabled_moves_stray_assignment_into_features_section() {
        let input = "\
[features]
other = true

[mcp_servers.lean-ctx]
command = \"lean-ctx\"
codex_hooks = true
";

        let output = ensure_codex_hooks_enabled(input)
            .expect("stray codex_hooks assignment should be normalized");

        assert!(output.contains("[features]\nother = true\nhooks = true\n"));
        assert_eq!(output.matches("hooks = true").count(), 1);
        assert!(!output.contains("[mcp_servers.lean-ctx]\ncommand = \"lean-ctx\"\nhooks = true"));
    }

    #[test]
    fn ensure_codex_hooks_enabled_adds_features_section_when_missing() {
        let input = "\
[mcp_servers.lean-ctx]
command = \"lean-ctx\"
";

        let output =
            ensure_codex_hooks_enabled(input).expect("missing features section should be added");

        assert!(output.ends_with("\n[features]\nhooks = true\n"));
    }

    #[test]
    fn ensure_mcp_server_adds_section_when_missing() {
        let input = "[features]\ncodex_hooks = true\n";
        let result = ensure_codex_mcp_server(input, "lean-ctx").expect("should add MCP section");
        assert!(result.contains("[mcp_servers.lean-ctx]"));
        assert!(result.contains("command = \"lean-ctx\""));
        assert!(result.contains("args = []"));
        assert!(result.contains("[features]\ncodex_hooks = true\n"));
    }

    #[test]
    fn ensure_mcp_server_noop_when_already_complete() {
        // Parent + args already present: the upsert must be a true no-op (no
        // churn on every session start).
        let input = "[mcp_servers.lean-ctx]\ncommand = \"lean-ctx\"\nargs = []\n";
        assert!(
            ensure_codex_mcp_server(input, "lean-ctx").is_none(),
            "should not modify config when MCP section already has all keys"
        );
    }

    #[test]
    fn ensure_mcp_server_preserves_existing_sections() {
        let input = "[mcp_servers.other]\ncommand = \"other\"\n";
        let result = ensure_codex_mcp_server(input, "/usr/bin/lean-ctx")
            .expect("should add lean-ctx section");
        assert!(result.contains("[mcp_servers.other]"));
        assert!(result.contains("[mcp_servers.lean-ctx]"));
        assert!(result.contains("command = \"/usr/bin/lean-ctx\""));
    }

    #[test]
    fn ensure_mcp_server_replaces_orphaned_env_subtable() {
        let input = "\
[mcp_servers.lean-ctx.env]
LEAN_CTX_DATA_DIR = \"/Users/user/.lean-ctx\"
";
        let result = ensure_codex_mcp_server(input, "/usr/local/bin/lean-ctx")
            .expect("should write the parent section");
        assert!(result.contains("[mcp_servers.lean-ctx]"));
        assert!(result.contains("command = \"/usr/local/bin/lean-ctx\""));
        assert!(
            !result.contains("[mcp_servers.lean-ctx.env]") && !result.contains("LEAN_CTX_DATA_DIR"),
            "a stale env pin must be dropped: {result}"
        );
    }

    #[test]
    fn ensure_mcp_server_handles_issue_189_scenario() {
        let input = "\
source = \"/Users/user/.cache/codex-runtimes/codex-primary-runtime/plugins/openai-primary-runtime\"
source_type = \"local\"

[mcp_servers.lean-ctx.env]
LEAN_CTX_DATA_DIR = \"/Users/user/.lean-ctx\"
";
        let result = ensure_codex_mcp_server(input, "/usr/local/bin/lean-ctx")
            .expect("should fix orphaned config from issue #189");
        assert!(result.contains("[mcp_servers.lean-ctx]\n"));
        assert!(result.contains("command = \"/usr/local/bin/lean-ctx\""));
        assert!(!result.contains("[mcp_servers.lean-ctx.env]"));
        // Original sibling content must survive the normalization.
        assert!(result.contains("source_type = \"local\""));
    }

    #[test]
    fn ensure_mcp_server_quotes_windows_backslash_paths() {
        let input = "[features]\ncodex_hooks = true\n";
        let win_path = r"C:\Users\Foo\AppData\Roaming\npm\lean-ctx.cmd";
        let result = ensure_codex_mcp_server(input, win_path).expect("should add MCP section");
        // Quote style is the TOML editor's concern; what matters is that the
        // backslash path round-trips to exactly the same string and stays valid.
        let doc = result
            .parse::<toml_edit::DocumentMut>()
            .expect("output must be valid TOML");
        assert_eq!(
            doc["mcp_servers"]["lean-ctx"]["command"].as_str(),
            Some(win_path),
            "Windows backslash path must round-trip exactly: {result}"
        );
    }

    #[test]
    fn ensure_mcp_server_does_not_match_similarly_named_section() {
        let input = "\
[mcp_servers.lean-ctx-other]
command = \"other\"
";
        let result = ensure_codex_mcp_server(input, "lean-ctx")
            .expect("should add lean-ctx section despite similarly-named section");
        assert!(result.contains("[mcp_servers.lean-ctx]\n"));
        assert!(result.contains("[mcp_servers.lean-ctx-other]"));
    }

    #[test]
    fn ensure_mcp_server_drops_stale_project_root_pin() {
        // Older installs baked LEAN_CTX_PROJECT_ROOT into the entry, pinning
        // one project for every Codex session. The upsert removes it, once.
        let input = "[mcp_servers.lean-ctx]\ncommand = \"lean-ctx\"\nargs = []\n\n\
                     [mcp_servers.lean-ctx.env]\nLEAN_CTX_PROJECT_ROOT = \"/work/main\"\n";
        let result =
            ensure_codex_mcp_server(input, "lean-ctx").expect("the stale pin must be removed");
        assert!(!result.contains("LEAN_CTX_PROJECT_ROOT"), "{result}");
        assert_eq!(result.matches("[mcp_servers.lean-ctx]").count(), 1);
        assert!(
            ensure_codex_mcp_server(&result, "lean-ctx").is_none(),
            "second pass must be a no-op"
        );
    }
}
