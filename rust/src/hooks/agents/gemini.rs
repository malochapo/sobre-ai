use super::super::{mcp_server_quiet_mode, resolve_hook_command_binary, write_file};

pub(crate) fn install_gemini_hook() {
    let Some(home) = crate::core::home::resolve_home_dir() else {
        tracing::error!("Cannot resolve home directory");
        return;
    };
    install_gemini_hook_config(&home);
}

/// Gemini CLI hooks in `~/.gemini/settings.json`. Gemini cannot replace a shell
/// command's output after it ran, so `BeforeTool` wraps the command in
/// `lean-ctx -c` — only when Gemini's own rules treat both forms alike, and
/// with no decision. Merged: only sobre's own entries are replaced.
pub(crate) fn install_gemini_hook_config(home: &std::path::Path) {
    let binary = resolve_hook_command_binary();
    let settings_path = home.join(".gemini").join("settings.json");
    let content = std::fs::read_to_string(&settings_path).unwrap_or_default();
    let mut root = if content.trim().is_empty() {
        serde_json::json!({})
    } else if let Ok(parsed) = crate::core::jsonc::parse_jsonc(&content) {
        parsed
    } else {
        return;
    };

    merge_gemini_hooks(&mut root, &binary);

    let formatted = serde_json::to_string_pretty(&root).unwrap_or_default();
    if formatted == content {
        return;
    }
    write_file(&settings_path, &formatted);
    if !mcp_server_quiet_mode() {
        eprintln!("Installed Gemini CLI hooks in {}", settings_path.display());
    }
}

fn merge_gemini_hooks(root: &mut serde_json::Value, binary: &str) {
    if !root.is_object() {
        *root = serde_json::json!({});
    }
    let Some(obj) = root.as_object_mut() else {
        return;
    };
    let hooks = obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };
    ensure_entry(
        hooks_obj,
        "BeforeTool",
        "run_shell_command",
        &format!("{binary} hook gemini"),
    );
    ensure_entry(
        hooks_obj,
        "AfterTool",
        ".*",
        &format!("{binary} hook observe"),
    );
}

fn ensure_entry(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    matcher: &str,
    command: &str,
) {
    let arr = hooks_obj
        .entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]));
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let Some(entries) = arr.as_array_mut() else {
        return;
    };
    entries.retain(|group| !is_sobre_group(group));
    entries.push(serde_json::json!({
        "matcher": matcher,
        "hooks": [{ "type": "command", "command": command }]
    }));
}

fn is_sobre_group(group: &serde_json::Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains("lean-ctx") && c.contains(" hook "))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_foreign_hooks_and_is_idempotent() {
        let mut v = serde_json::json!({
            "theme": "dark",
            "hooks": {
                "BeforeTool": [
                    { "matcher": "write_file", "hooks": [{ "type": "command", "command": "my-check" }] }
                ]
            }
        });
        merge_gemini_hooks(&mut v, "/bin/lean-ctx");
        merge_gemini_hooks(&mut v, "/bin/lean-ctx");

        assert_eq!(v["theme"], "dark");
        let before = v["hooks"]["BeforeTool"].as_array().unwrap();
        assert_eq!(before.len(), 2);
        assert_eq!(before[0]["hooks"][0]["command"], "my-check");
        assert_eq!(before[1]["matcher"], "run_shell_command");
        assert_eq!(
            before[1]["hooks"][0]["command"],
            "/bin/lean-ctx hook gemini"
        );
        assert_eq!(v["hooks"]["AfterTool"].as_array().unwrap().len(), 1);
    }
}
