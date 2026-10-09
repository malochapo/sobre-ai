use super::super::resolve_hook_command_binary;

pub(crate) fn install_gemini_hook_config(home: &std::path::Path) -> Result<(), String> {
    let binary = resolve_hook_command_binary();
    let settings_path = home.join(".gemini").join("settings.json");
    crate::config_io::edit_json(&settings_path, |root| {
        let mut value = serde_json::Value::Object(std::mem::take(root));
        merge_gemini_hooks(&mut value, &binary);
        if let serde_json::Value::Object(map) = value {
            *root = map;
        }
    })
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
                    .is_some_and(|c| c.contains("sobre") && c.contains(" hook "))
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
        merge_gemini_hooks(&mut v, "/bin/sobre");
        merge_gemini_hooks(&mut v, "/bin/sobre");

        assert_eq!(v["theme"], "dark");
        let before = v["hooks"]["BeforeTool"].as_array().unwrap();
        assert_eq!(before.len(), 2);
        assert_eq!(before[0]["hooks"][0]["command"], "my-check");
        assert_eq!(before[1]["matcher"], "run_shell_command");
        assert_eq!(before[1]["hooks"][0]["command"], "/bin/sobre hook gemini");
        assert_eq!(v["hooks"]["AfterTool"].as_array().unwrap().len(), 1);
    }
}
