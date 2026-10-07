use super::super::{mcp_server_quiet_mode, resolve_hook_command_binary, write_file};

/// Keep exactly one sobre `preToolUse` entry for `matcher`; other entries,
/// including the user's own hooks on the same matcher, are left alone.
fn ensure_pretooluse_hook(pre: &mut Vec<serde_json::Value>, matcher: &str, command: &str) {
    pre.retain(|e| {
        !e.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("lean-ctx") && c.contains(" hook "))
    });
    pre.push(serde_json::json!({ "matcher": matcher, "command": command }));
}

fn ensure_observe_hook(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    observe_cmd: &str,
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
    let already = entries.iter().any(|e| {
        e.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("hook observe"))
    });
    if !already {
        entries.push(serde_json::json!({ "command": observe_cmd }));
    }
}

fn merge_cursor_hooks(existing: &mut serde_json::Value, binary: &str) {
    if !existing.is_object() {
        *existing = serde_json::json!({});
    }
    let Some(root) = existing.as_object_mut() else {
        return;
    };
    root.insert("version".to_string(), serde_json::json!(1));

    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };

    // Cursor cannot replace a shell command's output after it ran, so the
    // command is wrapped in `lean-ctx -c` before it runs — only when Cursor's
    // own rules treat both forms alike, and with no permission decision.
    let pre = hooks_obj
        .entry("preToolUse".to_string())
        .or_insert_with(|| serde_json::json!([]));
    if !pre.is_array() {
        *pre = serde_json::json!([]);
    }
    let Some(pre_arr) = pre.as_array_mut() else {
        return;
    };
    ensure_pretooluse_hook(pre_arr, "Shell", &format!("{binary} hook cursor"));

    // Observe hooks — only essential ones (#1200). postToolUse caused
    // Cursor to append hook stdout to edited files, corrupting source code.
    let observe_cmd = format!("{binary} hook observe");
    ensure_observe_hook(hooks_obj, "sessionStart", &observe_cmd);
    ensure_observe_hook(hooks_obj, "preCompact", &observe_cmd);
}

pub fn install_cursor_hook() {
    let Some(home) = crate::core::home::resolve_home_dir() else {
        tracing::error!("Cannot resolve home directory");
        return;
    };

    install_cursor_hook_config(&home);

    if !mcp_server_quiet_mode() {
        eprintln!("Restart Cursor to activate.");
    }
}

pub(crate) fn install_cursor_hook_config(home: &std::path::Path) {
    let binary = resolve_hook_command_binary();
    let hooks_json = home.join(".cursor").join("hooks.json");

    let content = if hooks_json.exists() {
        std::fs::read_to_string(&hooks_json).unwrap_or_default()
    } else {
        String::new()
    };

    let mut existing = if content.trim().is_empty() {
        serde_json::json!({})
    } else {
        crate::core::jsonc::parse_jsonc(&content).unwrap_or_else(|_| serde_json::json!({}))
    };

    if !existing.is_object() {
        existing = serde_json::json!({});
    }

    // Merge-based: preserve other hooks/plugins. Only upsert lean-ctx entries.
    merge_cursor_hooks(&mut existing, &binary);

    let formatted = serde_json::to_string_pretty(&existing).unwrap_or_default();
    write_file(&hooks_json, &formatted);

    if !mcp_server_quiet_mode() {
        eprintln!("Installed Cursor hooks at {}", hooks_json.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_hooks_merge_preserves_other_entries() {
        let mut v = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "matcher": "Shell", "command": "/old/lean-ctx hook cursor" },
                    { "matcher": "Shell", "command": "my-own-check" },
                    { "matcher": "Other", "command": "do-something" }
                ],
                "postToolUse": [
                    { "matcher": "Shell", "command": "post" }
                ]
            },
            "otherKey": { "x": 1 }
        });

        merge_cursor_hooks(&mut v, "/new/lean-ctx");
        merge_cursor_hooks(&mut v, "/new/lean-ctx"); // re-run is idempotent

        assert!(v.get("otherKey").is_some());
        assert!(v.pointer("/hooks/postToolUse").is_some());
        let pre = v
            .pointer("/hooks/preToolUse")
            .and_then(|x| x.as_array())
            .unwrap();
        let commands: Vec<&str> = pre
            .iter()
            .filter_map(|e| e.get("command").and_then(|c| c.as_str()))
            .collect();
        assert_eq!(
            commands,
            ["my-own-check", "do-something", "/new/lean-ctx hook cursor"]
        );
    }
}
