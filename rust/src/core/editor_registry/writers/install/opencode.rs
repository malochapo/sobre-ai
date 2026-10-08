#[allow(clippy::wildcard_imports)]
use super::super::shared::*;
use super::super::{WriteAction, WriteOptions, WriteResult};
use crate::core::editor_registry::types::EditorTarget;

pub(crate) fn write_opencode_config(
    target: &EditorTarget,
    binary: &str,
    opts: WriteOptions,
) -> Result<WriteResult, String> {
    let desired = serde_json::json!({
        "type": "local",
        "command": [binary],
        "enabled": true
    });

    if target.config_path.exists() {
        let content = std::fs::read_to_string(&target.config_path).map_err(|e| e.to_string())?;
        let mut json = match crate::core::jsonc::parse_jsonc(&content) {
            Ok(v) => v,
            Err(_e) => {
                return handle_invalid_json_write(
                    &target.config_path,
                    &content,
                    "mcp",
                    "sobre",
                    &desired,
                    opts.overwrite_invalid,
                );
            }
        };
        let obj = json
            .as_object_mut()
            .ok_or_else(|| "root JSON must be an object".to_string())?;
        let mcp = obj.entry("mcp").or_insert_with(|| serde_json::json!({}));
        let mcp_obj = mcp
            .as_object_mut()
            .ok_or_else(|| "\"mcp\" must be an object".to_string())?;

        let existing = mcp_obj.get("sobre").cloned();
        if existing.as_ref() == Some(&desired) {
            return Ok(WriteResult {
                action: WriteAction::Already,
                note: None,
            });
        }
        mcp_obj.insert("sobre".to_string(), desired);

        let formatted = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
        crate::config_io::write_atomic_with_backup(&target.config_path, &formatted)?;
        return Ok(WriteResult {
            action: WriteAction::Updated,
            note: None,
        });
    }

    write_opencode_fresh(&target.config_path, binary, None)
}

pub(crate) fn write_opencode_fresh(
    path: &std::path::Path,
    binary: &str,
    note: Option<String>,
) -> Result<WriteResult, String> {
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": { "sobre": { "type": "local", "command": [binary], "enabled": true } }
    }))
    .map_err(|e| e.to_string())?;
    crate::config_io::write_atomic_with_backup(path, &content)?;
    Ok(WriteResult {
        action: if note.is_some() {
            WriteAction::Updated
        } else {
            WriteAction::Created
        },
        note,
    })
}
