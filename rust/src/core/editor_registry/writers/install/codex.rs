#[allow(clippy::wildcard_imports)]
use super::super::shared::*;
use super::super::{WriteAction, WriteResult};
use crate::core::editor_registry::types::EditorTarget;

pub(crate) fn write_codex_config(
    target: &EditorTarget,
    binary: &str,
) -> Result<WriteResult, String> {
    if target.config_path.exists() {
        let content = std::fs::read_to_string(&target.config_path).map_err(|e| e.to_string())?;
        let updated = upsert_codex_toml(&content, binary);
        if updated == content {
            return Ok(WriteResult {
                action: WriteAction::Already,
                note: None,
            });
        }
        crate::config_io::write_atomic_with_backup(&target.config_path, &updated)?;
        return Ok(WriteResult {
            action: WriteAction::Updated,
            note: None,
        });
    }

    let content = format!(
        "[mcp_servers.sobre]\ncommand = {}\nargs = []\n",
        toml_quote(binary)
    );
    crate::config_io::write_atomic_with_backup(&target.config_path, &content)?;
    Ok(WriteResult {
        action: WriteAction::Created,
        note: None,
    })
}

pub(crate) fn upsert_codex_toml(existing: &str, binary: &str) -> String {
    let mut out = String::with_capacity(existing.len() + 128);
    let mut in_section = false;
    let mut saw_section = false;
    let mut wrote_command = false;
    let mut wrote_args = false;

    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section && !wrote_command {
                out.push_str(&format!("command = {}\n", toml_quote(binary)));
                wrote_command = true;
            }
            if in_section && !wrote_args {
                out.push_str("args = []\n");
                wrote_args = true;
            }
            in_section = trimmed == "[mcp_servers.sobre]";
            if in_section {
                saw_section = true;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_section {
            if trimmed.starts_with("command") && trimmed.contains('=') {
                out.push_str(&format!("command = {}\n", toml_quote(binary)));
                wrote_command = true;
                continue;
            }
            if trimmed.starts_with("args") && trimmed.contains('=') {
                out.push_str("args = []\n");
                wrote_args = true;
                continue;
            }
        }

        out.push_str(line);
        out.push('\n');
    }

    if saw_section {
        if in_section && !wrote_command {
            out.push_str(&format!("command = {}\n", toml_quote(binary)));
        }
        if in_section && !wrote_args {
            out.push_str("args = []\n");
        }
        return out;
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n[mcp_servers.sobre]\n");
    out.push_str(&format!("command = {}\n", toml_quote(binary)));
    out.push_str("args = []\n");
    out
}
