//! Writes sobre's MCP server entry into each agent's config. One writer per
//! agent config: hooks and skills are installed elsewhere, and nothing else
//! writes this entry.

use serde_json::Value;

use super::types::{ConfigType, EditorTarget};

/// What a write did to the agent's config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    Created,
    Updated,
    Already,
}

/// Puts sobre's MCP server entry into `target`'s config, replacing an older
/// one. Other servers and settings are kept as the user wrote them.
pub fn write_config(target: &EditorTarget, binary: &str) -> Result<WriteAction, String> {
    if target.config_type == ConfigType::Codex {
        return write_codex_config(&target.config_path, binary);
    }
    let (container, entry) = json_entry(target.config_type, binary);
    if matches!(target.agent_key.as_str(), "claude" | "claude-code") {
        // Claude Code may store the entry in its own shape (`type`, `args`,
        // `env`); the command is what has to match.
        let current = current_entry(&target.config_path, container);
        if current.as_ref().and_then(|c| c.get("command")) == entry.get("command") {
            return Ok(WriteAction::Already);
        }
    }
    upsert_json_entry(&target.config_path, container, entry)
}

/// The key holding MCP servers in the host's JSON config, and sobre's entry
/// in the shape that host expects. Never an approval setting (`autoApprove`,
/// `trust`, …): what may run without asking is the host's call.
fn json_entry(config_type: ConfigType, binary: &str) -> (&'static str, Value) {
    match config_type {
        ConfigType::McpJson | ConfigType::GeminiSettings => {
            ("mcpServers", serde_json::json!({ "command": binary }))
        }
        ConfigType::Zed => (
            "context_servers",
            serde_json::json!({ "command": binary, "args": [], "env": {} }),
        ),
        ConfigType::VsCodeMcp => (
            "servers",
            serde_json::json!({ "type": "stdio", "command": binary, "args": [] }),
        ),
        ConfigType::CopilotCli => (
            "mcpServers",
            serde_json::json!({
                "type": "local",
                "command": binary,
                "args": ["mcp"],
                "tools": ["*"]
            }),
        ),
        ConfigType::OpenCode => (
            "mcp",
            serde_json::json!({ "type": "local", "command": [binary], "enabled": true }),
        ),
        ConfigType::Codex => unreachable!("Codex is TOML"),
    }
}

fn current_entry(path: &std::path::Path, container: &str) -> Option<Value> {
    let content = std::fs::read_to_string(path).ok()?;
    crate::core::jsonc::parse_jsonc(&content)
        .ok()?
        .get(container)?
        .get("sobre")
        .cloned()
}

fn upsert_json_entry(
    path: &std::path::Path,
    container: &str,
    entry: Value,
) -> Result<WriteAction, String> {
    let existed = path.exists();
    if current_entry(path, container).as_ref() == Some(&entry) {
        return Ok(WriteAction::Already);
    }
    let mut container_is_object = true;
    crate::config_io::edit_json(path, |root| {
        let servers = root
            .entry(container)
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        match servers.as_object_mut() {
            Some(servers) => {
                servers.insert("sobre".to_string(), entry);
            }
            None => container_is_object = false,
        }
    })?;
    if !container_is_object {
        return Err(format!(
            "\"{container}\" in {} is not an object; left unchanged",
            path.display()
        ));
    }
    Ok(if existed {
        WriteAction::Updated
    } else {
        WriteAction::Created
    })
}

/// Sets `[mcp_servers.sobre]` in Codex's `config.toml`, keeping every other
/// table, key and comment as written.
fn write_codex_config(path: &std::path::Path, binary: &str) -> Result<WriteAction, String> {
    let existed = path.exists();
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let updated = upsert_codex_toml(&content, binary)
        .map_err(|e| format!("{} is not valid TOML ({e}); left unchanged", path.display()))?;
    if updated == content {
        return Ok(WriteAction::Already);
    }
    crate::config_io::write_atomic_with_backup(path, &updated)?;
    Ok(if existed {
        WriteAction::Updated
    } else {
        WriteAction::Created
    })
}

pub(crate) fn upsert_codex_toml(existing: &str, binary: &str) -> Result<String, String> {
    let mut doc = existing
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    // `[mcp_servers]` stays implicit so no bare parent header is emitted.
    let servers = doc["mcp_servers"].or_insert(toml_edit::table());
    let servers = servers.as_table_mut().ok_or("mcp_servers is not a table")?;
    servers.set_implicit(true);
    let sobre = servers["sobre"].or_insert(toml_edit::table());
    let sobre = sobre
        .as_table_mut()
        .ok_or("mcp_servers.sobre is not a table")?;
    sobre.set_implicit(false);
    let command_matches = sobre.get("command").and_then(|c| c.as_str()) == Some(binary);
    if !command_matches {
        sobre["command"] = toml_edit::value(binary);
    }
    let args_empty = sobre
        .get("args")
        .and_then(|a| a.as_array())
        .is_some_and(toml_edit::Array::is_empty);
    if !args_empty {
        sobre["args"] = toml_edit::value(toml_edit::Array::new());
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests;
