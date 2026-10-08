use super::shared::{handle_invalid_json_write, try_text_inject_mcp_entry};
use super::*;
use serde_json::Value;
use std::path::PathBuf;

fn target(name: &'static str, path: PathBuf, ty: ConfigType) -> EditorTarget {
    EditorTarget {
        name,
        agent_key: "test".to_string(),
        config_path: path,
        detect_path: PathBuf::from("/nonexistent"),
        config_type: ty,
    }
}

#[test]
fn mcp_json_upserts_and_preserves_other_servers_without_approval_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(
            &path,
            r#"{ "mcpServers": { "other": { "command": "other-bin" }, "lean-ctx": { "command": "/old/path/lean-ctx", "autoApprove": [] } } }"#,
        )
        .unwrap();

    let t = target("test", path.clone(), ConfigType::McpJson);
    let res = write_mcp_json(&t, "/new/path/lean-ctx", WriteOptions::default()).unwrap();
    assert_eq!(res.action, WriteAction::Updated);

    let json: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(json["mcpServers"]["other"]["command"], "other-bin");
    assert_eq!(
        json["mcpServers"]["lean-ctx"]["command"],
        "/new/path/lean-ctx"
    );
    assert!(json["mcpServers"]["lean-ctx"].get("autoApprove").is_none());
}

#[test]
fn codex_toml_upserts_existing_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"[mcp_servers.lean-ctx]
command = "old"
args = ["x"]
"#,
    )
    .unwrap();

    let t = target("test", path.clone(), ConfigType::Codex);
    let res = write_codex_config(&t, "new").unwrap();
    assert_eq!(res.action, WriteAction::Updated);

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains(r#"command = "new""#));
    assert!(content.contains("args = []"));
}

#[test]
fn upsert_codex_toml_inserts_new_section_when_missing() {
    let updated = upsert_codex_toml("[other]\nx=1\n", "lean-ctx");
    assert!(updated.contains("[mcp_servers.lean-ctx]"));
    assert!(updated.contains("command = \"lean-ctx\""));
    assert!(updated.contains("args = []"));
}

#[test]
fn codex_toml_uses_single_quotes_for_backslash_paths() {
    let win_path = r"C:\Users\Foo\AppData\Roaming\npm\lean-ctx.cmd";
    let updated = upsert_codex_toml("", win_path);
    assert!(
        updated.contains(&format!("command = '{win_path}'")),
        "Windows paths must use TOML single quotes to avoid backslash escapes: {updated}"
    );
}

#[test]
fn codex_toml_uses_double_quotes_for_unix_paths() {
    let unix_path = "/usr/local/bin/lean-ctx";
    let updated = upsert_codex_toml("", unix_path);
    assert!(
        updated.contains(&format!("command = \"{unix_path}\"")),
        "Unix paths should use double quotes: {updated}"
    );
}

#[test]
fn upsert_codex_toml_does_not_duplicate_parent_when_present() {
    let input = "\
[mcp_servers.lean-ctx]
command = \"old\"
args = [\"x\"]

[mcp_servers.lean-ctx.env]
FOO = \"bar\"
";
    let updated = upsert_codex_toml(input, "new");
    assert_eq!(
        updated.matches("[mcp_servers.lean-ctx]").count(),
        1,
        "must not duplicate parent section"
    );
    assert!(updated.contains("command = \"new\""));
    assert!(updated.contains("args = []"));
    assert!(updated.contains("[mcp_servers.lean-ctx.env]\nFOO = \"bar\""));
}

#[test]
fn windsurf_config_omits_auto_approve() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp_config.json");

    let t = EditorTarget {
        name: "Windsurf",
        agent_key: "windsurf".to_string(),
        config_path: path.clone(),
        detect_path: PathBuf::from("/nonexistent"),
        config_type: ConfigType::McpJson,
    };
    let res = write_mcp_json(&t, "/usr/local/bin/lean-ctx", WriteOptions::default()).unwrap();
    assert_eq!(res.action, WriteAction::Created);

    let json: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(json["mcpServers"]["lean-ctx"]["autoApprove"].is_null());
    assert_eq!(
        json["mcpServers"]["lean-ctx"]["command"],
        "/usr/local/bin/lean-ctx"
    );
}

#[test]
fn text_inject_into_existing_mcp_object() {
    let content = r#"{
  "mcp": {}
}"#;
    let value = serde_json::json!({"type": "local", "command": ["lean-ctx"]});
    let result = try_text_inject_mcp_entry(content, "mcp", "lean-ctx", &value);
    assert!(result.is_some());
    let patched = result.unwrap();
    assert!(patched.contains("\"lean-ctx\""));
    assert!(patched.contains("\"type\": \"local\""));
}

#[test]
fn text_inject_creates_container_when_missing() {
    let content = r#"{
  "some_other_key": "value"
}"#;
    let value = serde_json::json!({"command": "lean-ctx"});
    // For mcpServers container
    let result = try_text_inject_mcp_entry(content, "mcpServers", "lean-ctx", &value);
    assert!(result.is_some());
    let patched = result.unwrap();
    assert!(patched.contains("\"mcpServers\""));
    assert!(patched.contains("\"lean-ctx\""));

    // For mcp container (OpenCode)
    let result2 = try_text_inject_mcp_entry(content, "mcp", "lean-ctx", &value);
    assert!(result2.is_some());
    let patched2 = result2.unwrap();
    assert!(patched2.contains("\"mcp\""));
    assert!(patched2.contains("\"lean-ctx\""));

    // For context_servers container (Zed)
    let result3 = try_text_inject_mcp_entry(content, "context_servers", "lean-ctx", &value);
    assert!(result3.is_some());
    let patched3 = result3.unwrap();
    assert!(patched3.contains("\"context_servers\""));
    assert!(patched3.contains("\"lean-ctx\""));
}

#[test]
fn text_inject_into_populated_mcp_object() {
    let content = r#"{
  "mcp": {
    "other-server": {"type": "local"}
  }
}"#;
    let value = serde_json::json!({"type": "local", "command": ["lean-ctx"]});
    let result = try_text_inject_mcp_entry(content, "mcp", "lean-ctx", &value);
    assert!(result.is_some());
    let patched = result.unwrap();
    assert!(patched.contains("\"lean-ctx\""));
    assert!(patched.contains("\"other-server\""));
}

#[test]
fn handle_invalid_json_skips_when_entry_already_present() {
    let content = r#"{ invalid json "lean-ctx": stuff }"#;
    let value = serde_json::json!({"type": "local"});
    let result = handle_invalid_json_write(
        std::path::Path::new("/tmp/test.json"),
        content,
        "mcp",
        "lean-ctx",
        &value,
        true,
    );
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r.action, WriteAction::Already);
}

#[test]
fn handle_invalid_json_returns_error_when_inject_disabled() {
    let content = r"{ invalid json without key }";
    let value = serde_json::json!({"type": "local"});
    let result = handle_invalid_json_write(
        std::path::Path::new("/tmp/test.json"),
        content,
        "mcp",
        "lean-ctx",
        &value,
        false,
    );
    assert!(result.is_err());
}

#[test]
fn handle_invalid_json_does_not_overwrite_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    let invalid_content = r#"{ "mcp": { BROKEN "other": true } }"#;
    std::fs::write(&path, invalid_content).unwrap();

    let value = serde_json::json!({"type": "local", "command": ["lean-ctx"]});
    let result = handle_invalid_json_write(&path, invalid_content, "mcp", "lean-ctx", &value, true);
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r.action, WriteAction::Updated);

    // Original file should still exist (not deleted/renamed)
    let final_content = std::fs::read_to_string(&path).unwrap();
    assert!(
        final_content.contains("lean-ctx"),
        "lean-ctx should be injected"
    );
    assert!(
        final_content.contains("BROKEN"),
        "original content preserved"
    );
}
