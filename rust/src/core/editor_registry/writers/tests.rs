use super::*;
use std::path::PathBuf;

fn target(agent_key: &str, path: PathBuf, ty: ConfigType) -> EditorTarget {
    EditorTarget {
        name: "test",
        agent_key: agent_key.to_string(),
        config_path: path,
        detect_path: PathBuf::from("/nonexistent"),
        config_type: ty,
    }
}

fn read(path: &std::path::Path) -> Value {
    crate::core::jsonc::parse_jsonc(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn mcp_json_replaces_sobre_and_keeps_other_servers_without_approval_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(
        &path,
        r#"{ "mcpServers": { "other": { "command": "other-bin" }, "sobre": { "command": "/old/path/sobre", "autoApprove": [] } } }"#,
    )
    .unwrap();

    let t = target("cursor", path.clone(), ConfigType::McpJson);
    assert_eq!(
        write_config(&t, "/new/path/sobre").unwrap(),
        WriteAction::Updated
    );
    assert_eq!(
        write_config(&t, "/new/path/sobre").unwrap(),
        WriteAction::Already
    );

    let json = read(&path);
    assert_eq!(json["mcpServers"]["other"]["command"], "other-bin");
    assert_eq!(json["mcpServers"]["sobre"]["command"], "/new/path/sobre");
    assert!(json["mcpServers"]["sobre"].get("autoApprove").is_none());
}

#[test]
fn json_configs_keep_comments_and_key_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let original = "{\n  // my theme\n  \"theme\": \"One Dark\",\n  \"context_servers\": {\n    \"other\": { \"command\": \"x\" },\n  },\n  \"vim_mode\": true,\n}\n";
    std::fs::write(&path, original).unwrap();

    let t = target("zed", path.clone(), ConfigType::Zed);
    assert_eq!(
        write_config(&t, "/bin/sobre").unwrap(),
        WriteAction::Updated
    );

    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("// my theme"), "{out}");
    assert!(out.find("\"theme\"").unwrap() < out.find("\"vim_mode\"").unwrap());
    assert_eq!(
        read(&path)["context_servers"]["sobre"]["command"],
        "/bin/sobre"
    );
    assert_eq!(read(&path)["context_servers"]["other"]["command"], "x");
}

#[test]
fn a_missing_config_is_created_with_only_the_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/opencode.json");

    let t = target("opencode", path.clone(), ConfigType::OpenCode);
    assert_eq!(
        write_config(&t, "/bin/sobre").unwrap(),
        WriteAction::Created
    );
    assert_eq!(
        read(&path),
        serde_json::json!({
            "mcp": { "sobre": { "type": "local", "command": ["/bin/sobre"], "enabled": true } }
        })
    );
}

#[test]
fn an_unparseable_config_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    let broken = r#"{ "mcpServers": { BROKEN "other": true } "#;
    std::fs::write(&path, broken).unwrap();

    let t = target("cursor", path.clone(), ConfigType::McpJson);
    let err = write_config(&t, "/bin/sobre").unwrap_err();
    assert!(err.contains("left unchanged"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}

#[test]
fn codex_toml_upserts_existing_section_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "model = \"o3\" # mine\n\n[mcp_servers.sobre]\ncommand = \"old\"\nargs = [\"x\"]\n\n[mcp_servers.sobre.env]\nFOO = \"bar\"\n",
    )
    .unwrap();

    let t = target("codex", path.clone(), ConfigType::Codex);
    assert_eq!(write_config(&t, "new").unwrap(), WriteAction::Updated);
    assert_eq!(write_config(&t, "new").unwrap(), WriteAction::Already);

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("model = \"o3\" # mine"), "{content}");
    assert_eq!(content.matches("[mcp_servers.sobre]").count(), 1);
    assert!(content.contains("[mcp_servers.sobre.env]\nFOO = \"bar\""));
    let doc: toml::Value = toml::from_str(&content).unwrap();
    assert_eq!(doc["mcp_servers"]["sobre"]["command"].as_str(), Some("new"));
    assert_eq!(
        doc["mcp_servers"]["sobre"]["args"].as_array().map(Vec::len),
        Some(0)
    );
}

#[test]
fn codex_toml_round_trips_windows_paths() {
    let win_path = r"C:\Users\Foo\AppData\Roaming\npm\sobre.cmd";
    let updated = upsert_codex_toml("[other]\nx = 1\n", win_path).unwrap();
    assert!(updated.starts_with("[other]\nx = 1\n"), "{updated}");
    let doc: toml::Value = toml::from_str(&updated).unwrap();
    assert_eq!(
        doc["mcp_servers"]["sobre"]["command"].as_str(),
        Some(win_path)
    );
}

#[test]
fn check_mode_records_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");

    crate::config_io::journal::begin(true);
    let t = target("windsurf", path.clone(), ConfigType::McpJson);
    let action = write_config(&t, "/bin/sobre");
    let recorded = crate::config_io::journal::take();
    crate::config_io::journal::end();

    assert_eq!(action.unwrap(), WriteAction::Created);
    assert!(!path.exists());
    assert_eq!(
        recorded,
        vec![(path, crate::config_io::journal::Change::Created)]
    );
}
