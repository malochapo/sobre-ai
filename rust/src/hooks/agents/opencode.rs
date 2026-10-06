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
        "enabled": true,
        "environment": super::super::mcp_server_env_json()
    });

    // #313: native tools (read, grep, glob, bash) are denied at the permission
    // level in opencode.json, forcing the agent to use ctx_* equivalents via the
    // MCP server.

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

    // 1. Apply shadow permissions
    let perm_changed = apply_shadow_permissions_inplace(obj);

    // 2. Register MCP server (if needed and not already present)
    let mcp_written = if !has_lean_ctx {
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
    } else {
        false
    };

    // 3. Single write if anything changed
    if perm_changed || mcp_written {
        if file_existed {
            let backup = config_path.with_extension("json.bak");
            let _ = std::fs::copy(&config_path, &backup);
        }
        if let Ok(formatted) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(&config_path, formatted);
            if !mcp_server_quiet_mode() {
                if perm_changed {
                    eprintln!(
                        "  \x1b[32m✓\x1b[0m Shadow mode: native tools denied at {display_path}"
                    );
                }
                if mcp_written {
                    eprintln!("  \x1b[32m✓\x1b[0m OpenCode MCP configured at {display_path}");
                }
            }
        }
    } else if has_lean_ctx && !mcp_server_quiet_mode() {
        eprintln!("OpenCode MCP already configured at {display_path}");
    }

    // #442: inject the "prefer ctx_*" rules block so the agent knows to use
    // lean-ctx tools. In shadow mode, native tools are denied — the agent
    // must use ctx_* tools, so rules are even more important.
    let _ = crate::rules_inject::inject_rules_for_agent(&home, "OpenCode");
}

/// Native tools that shadow mode denies via opencode.json `permission` object.
const SHADOW_DENIED_TOOLS: &[&str] = &["read", "grep", "glob", "bash"];

/// Apply permission denies in-place on a JSON object.
/// Returns true if any changes were made.
fn apply_shadow_permissions_inplace(obj: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let perms = obj
        .entry("permission".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let Some(perms_obj) = perms.as_object_mut() else {
        return false;
    };

    let mut changed = false;
    for &tool in SHADOW_DENIED_TOOLS {
        if perms_obj.get(tool).and_then(|v| v.as_str()) != Some("deny") {
            perms_obj.insert(tool.to_string(), serde_json::json!("deny"));
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(json: &str) -> (bool, serde_json::Value) {
        let mut v: serde_json::Value = serde_json::from_str(json).unwrap();
        let changed = apply_shadow_permissions_inplace(v.as_object_mut().unwrap());
        (changed, v)
    }

    #[test]
    fn apply_adds_deny_for_all_tools_and_keeps_other_keys() {
        let (changed, json) = apply(r#"{"mcp":{"other":{"type":"local"}}}"#);
        assert!(changed);
        for tool in SHADOW_DENIED_TOOLS {
            assert_eq!(json["permission"][*tool], "deny", "{tool} should be deny");
        }
        assert_eq!(json["mcp"]["other"]["type"], "local");
    }

    #[test]
    fn apply_overwrites_user_allow_but_keeps_other_tools() {
        let (_, json) = apply(r#"{"permission":{"read":"allow","edit":"allow"}}"#);
        assert_eq!(json["permission"]["read"], "deny");
        assert_eq!(json["permission"]["edit"], "allow");
    }

    #[test]
    fn apply_is_idempotent() {
        let (_, first) = apply("{}");
        let (changed, second) = apply(&first.to_string());
        assert!(!changed, "a second apply must report no change");
        assert_eq!(first, second);
    }

    #[test]
    fn permission_key_is_singular() {
        let (_, json) = apply("{}");
        assert!(json.get("permission").is_some());
        assert!(json.get("permissions").is_none());
    }
}
