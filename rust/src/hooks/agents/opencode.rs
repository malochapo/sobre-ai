use super::super::{mcp_server_quiet_mode, resolve_binary_path};

pub(crate) fn install_opencode_hook() {
    let binary = resolve_binary_path();
    let home = crate::core::home::resolve_home_dir().unwrap_or_default();
    let config_path = home.join(".config/opencode/opencode.json");
    let display_path = "~/.config/opencode/opencode.json";

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let desired = serde_json::json!({
        "type": "local",
        "command": [&binary],
        "enabled": true
    });

    let file_existed = config_path.exists();
    let content = if file_existed {
        std::fs::read_to_string(&config_path).unwrap_or_default()
    } else {
        String::new()
    };
    let has_lean_ctx = content.contains("lean-ctx");

    let mut json = crate::core::jsonc::parse_jsonc(&content)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    let Some(obj) = json.as_object_mut() else {
        return;
    };

    let mcp_written = if has_lean_ctx {
        false
    } else {
        if !file_existed {
            obj.insert(
                "$schema".to_string(),
                serde_json::json!("https://opencode.ai/config.json"),
            );
        }
        let mcp = obj.entry("mcp").or_insert_with(|| serde_json::json!({}));
        if let Some(mcp_obj) = mcp.as_object_mut() {
            mcp_obj.insert("lean-ctx".to_string(), desired);
            true
        } else {
            false
        }
    };

    if mcp_written {
        if file_existed {
            let backup = config_path.with_extension("json.bak");
            let _ = std::fs::copy(&config_path, &backup);
        }
        if let Ok(formatted) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(&config_path, formatted);
            if !mcp_server_quiet_mode() {
                eprintln!("  \x1b[32m✓\x1b[0m OpenCode MCP configured at {display_path}");
            }
        }
    } else if has_lean_ctx && !mcp_server_quiet_mode() {
        eprintln!("OpenCode MCP already configured at {display_path}");
    }
}
