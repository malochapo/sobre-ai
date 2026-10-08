use super::super::resolve_hook_command_binary;

/// Keep exactly one sobre `preToolUse` entry for `matcher`; other entries,
/// including the user's own hooks on the same matcher, are left alone.
fn ensure_pretooluse_hook(pre: &mut Vec<serde_json::Value>, matcher: &str, command: &str) {
    pre.retain(|e| {
        !e.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("sobre") && c.contains(" hook "))
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
    // command is wrapped in `sobre -c` before it runs — only when Cursor's
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

    // Observe hooks — only essential ones. postToolUse caused
    // Cursor to append hook stdout to edited files, corrupting source code.
    let observe_cmd = format!("{binary} hook observe");
    ensure_observe_hook(hooks_obj, "sessionStart", &observe_cmd);
    ensure_observe_hook(hooks_obj, "preCompact", &observe_cmd);
}

pub(crate) fn install_cursor_hook_config(home: &std::path::Path) -> Result<(), String> {
    let binary = resolve_hook_command_binary();
    let hooks_json = home.join(".cursor").join("hooks.json");
    crate::config_io::edit_json(&hooks_json, |root| {
        let mut value = serde_json::Value::Object(std::mem::take(root));
        merge_cursor_hooks(&mut value, &binary);
        if let serde_json::Value::Object(map) = value {
            *root = map;
        }
    })
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
                    { "matcher": "Shell", "command": "/old/sobre hook cursor" },
                    { "matcher": "Shell", "command": "my-own-check" },
                    { "matcher": "Other", "command": "do-something" }
                ],
                "postToolUse": [
                    { "matcher": "Shell", "command": "post" }
                ]
            },
            "otherKey": { "x": 1 }
        });

        merge_cursor_hooks(&mut v, "/new/sobre");
        merge_cursor_hooks(&mut v, "/new/sobre"); // re-run is idempotent

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
            ["my-own-check", "do-something", "/new/sobre hook cursor"]
        );
    }
}
