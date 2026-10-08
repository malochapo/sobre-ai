#[allow(clippy::wildcard_imports)]
use super::super::shared::*;
use super::super::{WriteAction, WriteOptions, WriteResult};
use crate::core::editor_registry::types::EditorTarget;

pub(crate) fn write_gemini_settings(
    target: &EditorTarget,
    binary: &str,
    opts: WriteOptions,
) -> Result<WriteResult, String> {
    let entry = serde_json::json!({ "command": binary });

    if target.config_path.exists() {
        let content = std::fs::read_to_string(&target.config_path).map_err(|e| e.to_string())?;
        let mut json = match crate::core::jsonc::parse_jsonc(&content) {
            Ok(v) => v,
            Err(_e) => {
                return handle_invalid_json_write(
                    &target.config_path,
                    &content,
                    "mcpServers",
                    "sobre",
                    &entry,
                    opts.overwrite_invalid,
                );
            }
        };
        let obj = json
            .as_object_mut()
            .ok_or_else(|| "root JSON must be an object".to_string())?;
        let servers = obj
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        let servers_obj = servers
            .as_object_mut()
            .ok_or_else(|| "\"mcpServers\" must be an object".to_string())?;

        let existing = servers_obj.get("sobre").cloned();
        if existing.as_ref() == Some(&entry) {
            return Ok(WriteResult {
                action: WriteAction::Already,
                note: None,
            });
        }
        servers_obj.insert("sobre".to_string(), entry);

        let formatted = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
        crate::config_io::write_atomic_with_backup(&target.config_path, &formatted)?;
        return Ok(WriteResult {
            action: WriteAction::Updated,
            note: None,
        });
    }

    let config = serde_json::json!({ "mcpServers": { "sobre": entry } });
    let formatted = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    crate::config_io::write_atomic_with_backup(&target.config_path, &formatted)?;
    Ok(WriteResult {
        action: WriteAction::Created,
        note: None,
    })
}
