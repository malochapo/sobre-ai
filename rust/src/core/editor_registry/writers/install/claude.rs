use serde_json::Value;

#[allow(clippy::wildcard_imports)]
use super::super::shared::*;
use super::super::{WriteAction, WriteOptions, WriteResult};
use crate::core::editor_registry::types::EditorTarget;

pub(crate) fn write_mcp_json(
    target: &EditorTarget,
    binary: &str,
    opts: WriteOptions,
) -> Result<WriteResult, String> {
    let include_aa = supports_auto_approve(target);
    let desired = if target.agent_key.is_empty() {
        lean_ctx_server_entry(binary, include_aa)
    } else {
        lean_ctx_server_entry_with_instructions(binary, include_aa, &target.agent_key)
    };

    // Claude Code manages ~/.claude.json and may overwrite it on first start.
    // Prefer the official CLI integration when available.
    // Skip when LEAN_CTX_QUIET=1 (bootstrap --json / setup --json) to avoid
    // spawning `claude mcp add-json` which can stall in non-interactive CI.
    if (target.agent_key == "claude" || target.name == "Claude Code")
        && !matches!(std::env::var("LEAN_CTX_QUIET"), Ok(v) if v.trim() == "1")
        && let Ok(result) = try_claude_mcp_add(&desired)
    {
        return Ok(result);
    }

    if target.config_path.exists() {
        let content = std::fs::read_to_string(&target.config_path).map_err(|e| e.to_string())?;
        let mut json = match crate::core::jsonc::parse_jsonc(&content) {
            Ok(v) => v,
            Err(_e) => {
                return handle_invalid_json_write(
                    &target.config_path,
                    &content,
                    "mcpServers",
                    "lean-ctx",
                    &desired,
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

        let existing = servers_obj.get("lean-ctx").cloned();
        if existing.as_ref() == Some(&desired) {
            return Ok(WriteResult {
                action: WriteAction::Already,
                note: None,
            });
        }
        servers_obj.insert("lean-ctx".to_string(), desired);

        let formatted = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
        crate::config_io::write_atomic_with_backup(&target.config_path, &formatted)?;
        return Ok(WriteResult {
            action: WriteAction::Updated,
            note: None,
        });
    }

    write_mcp_json_fresh(&target.config_path, &desired, None)
}

pub(crate) fn find_in_path(binary: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn validate_claude_binary() -> Result<std::path::PathBuf, String> {
    let path = find_in_path("claude").ok_or("claude binary not found in PATH")?;

    std::fs::canonicalize(&path).map_err(|e| format!("cannot resolve claude path: {e}"))
}

pub(crate) fn try_claude_mcp_add(desired: &Value) -> Result<WriteResult, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let server_json = serde_json::to_string(desired).map_err(|e| e.to_string())?;

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args([
            "/C", "claude", "mcp", "add-json", "--scope", "user", "lean-ctx",
        ]);
        c
    } else {
        let claude_path = validate_claude_binary()?;
        let mut c = Command::new(claude_path);
        c.args(["mcp", "add-json", "--scope", "user", "lean-ctx"]);
        c
    };

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(server_json.as_bytes());
    }

    let deadline = Duration::from_secs(3);
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(WriteResult {
                        action: WriteAction::Updated,
                        note: Some("via claude mcp add-json".to_string()),
                    })
                } else {
                    Err("claude mcp add-json failed".to_string())
                };
            }
            Ok(None) => {
                if start.elapsed() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("claude mcp add-json timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

pub(crate) fn write_mcp_json_fresh(
    path: &std::path::Path,
    desired: &Value,
    note: Option<String>,
) -> Result<WriteResult, String> {
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": { "lean-ctx": desired }
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
