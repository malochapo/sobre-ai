use std::path::PathBuf;

use super::super::{
    mcp_server_quiet_mode, resolve_binary_path, resolve_hook_command_binary,
    to_bash_compatible_path, write_file,
};

pub(crate) fn install_copilot_hook() {
    let binary = resolve_binary_path();

    // Copilot CLI loads its MCP servers from `~/.copilot/mcp-config.json`, and
    // VS Code from its User `mcp.json`. sobre installs at user level only —
    // never into a repository.
    write_copilot_cli_home_mcp();

    let mcp_path = crate::core::editor_registry::vscode_mcp_path();
    if mcp_path.as_os_str() == "/nonexistent" {
        if !mcp_server_quiet_mode() {
            eprintln!("  \x1b[2mVS Code not found — skipping VS Code Copilot config\x1b[0m");
        }
        return;
    }
    write_vscode_mcp_file(&mcp_path, &binary, "global VS Code User MCP");
    install_copilot_hooks();
}

/// Register lean-ctx in the Copilot CLI's global MCP config at
/// `~/.copilot/mcp-config.json`. Reuses the canonical `CopilotCli` writer so the
/// entry format and merge behavior match `configure_agent_mcp`, and uses the
/// portable binary path so repeated installs stay idempotent (no churn).
fn write_copilot_cli_home_mcp() {
    let Some(home) = dirs::home_dir() else {
        return;
    };

    let binary = crate::core::portable_binary::resolve_portable_binary();
    let target = crate::core::editor_registry::EditorTarget {
        name: "Copilot CLI",
        agent_key: "copilot".to_string(),
        detect_path: PathBuf::from("/nonexistent"),
        config_path: home.join(".copilot/mcp-config.json"),
        config_type: crate::core::editor_registry::ConfigType::CopilotCli,
    };

    match crate::core::editor_registry::write_config_with_options(
        &target,
        &binary,
        crate::core::editor_registry::WriteOptions {
            overwrite_invalid: true,
        },
    ) {
        Ok(result) => {
            if !mcp_server_quiet_mode() {
                use crate::core::editor_registry::WriteAction;
                let label = "~/.copilot/mcp-config.json";
                let msg = match result.action {
                    WriteAction::Created => format!("Created {label} with lean-ctx MCP server"),
                    WriteAction::Updated => format!("Added lean-ctx to {label}"),
                    WriteAction::Already => format!("lean-ctx already configured in {label}"),
                };
                eprintln!("  \x1b[32m✓\x1b[0m {msg}");
            }
        }
        Err(e) => {
            if !mcp_server_quiet_mode() {
                eprintln!(
                    "  \x1b[33m⚠\x1b[0m  Could not configure {}: {e}",
                    target.config_path.display()
                );
            }
        }
    }
}

/// One Copilot hook entry. Copilot CLI runs the `bash` field on Unix and the
/// `powershell` field on Windows (#381) — an entry carrying only `bash` has no
/// runnable command on Windows, so the hook errors and the CLI rejects the tool
/// call. Both commands quote the binary: Windows install paths routinely
/// contain spaces (`C:\Users\Jane Doe\AppData\...`).
fn copilot_hook_entry(binary: &str, action: &str, timeout_sec: u64) -> serde_json::Value {
    let bash_binary = to_bash_compatible_path(binary);
    serde_json::json!({
        "type": "command",
        "bash": format!("\"{bash_binary}\" hook {action}"),
        "powershell": format!("& \"{binary}\" hook {action}"),
        "timeoutSec": timeout_sec
    })
}

/// User-level hooks directory of the Copilot CLI: `$COPILOT_HOME/hooks/` when
/// set, else `~/.copilot/hooks/`. Note `~/.github/hooks/` is *not* read by
/// Copilot — only the repo-level `.github/hooks/` is (#381).
fn copilot_user_hooks_dir() -> Option<PathBuf> {
    if let Ok(copilot_home) = std::env::var("COPILOT_HOME") {
        let trimmed = copilot_home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("hooks"));
        }
    }
    crate::core::home::resolve_home_dir().map(|h| h.join(".copilot").join("hooks"))
}

/// Copilot CLI user hooks: shell output is compressed after the command ran
/// (`postToolUse` `modifiedResult`), never rewritten before. Merged into an
/// existing `hooks.json`: only sobre's own entries are replaced.
fn install_copilot_hooks() {
    let binary = resolve_hook_command_binary();
    let mut compress = copilot_hook_entry(&binary, "compress", 15);
    compress["matcher"] = serde_json::json!("bash|powershell");
    let observe = copilot_hook_entry(&binary, "observe", 5);

    let Some(dir) = copilot_user_hooks_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let hook_path = dir.join("hooks.json");

    let content = std::fs::read_to_string(&hook_path).unwrap_or_default();
    let mut root = if content.trim().is_empty() {
        serde_json::json!({})
    } else if let Ok(parsed) = crate::core::jsonc::parse_jsonc(&content) {
        parsed
    } else {
        return;
    };
    let Some(obj) = root.as_object_mut() else {
        return;
    };
    obj.insert("version".to_string(), serde_json::json!(1));
    let hooks = obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };
    let post = hooks_obj
        .entry("postToolUse".to_string())
        .or_insert_with(|| serde_json::json!([]));
    let Some(post_arr) = post.as_array_mut() else {
        return;
    };
    post_arr.retain(|e| !is_sobre_entry(e));
    post_arr.push(compress);
    post_arr.push(observe);

    let formatted = serde_json::to_string_pretty(&root).unwrap_or_default();
    if formatted == content {
        return;
    }
    write_file(&hook_path, &formatted);
    if !mcp_server_quiet_mode() {
        eprintln!("Installed Copilot hooks at {}", hook_path.display());
    }
}

/// An entry this installer wrote (`… hook <action>` on the sobre binary).
fn is_sobre_entry(entry: &serde_json::Value) -> bool {
    ["bash", "powershell"].iter().any(|k| {
        entry
            .get(*k)
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("lean-ctx") && c.contains(" hook "))
    })
}

fn server_entry(binary: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": binary,
        "args": []
    })
}

/// VS Code uses `"servers"` as the top-level key (not `"mcpServers"`).
fn write_vscode_mcp_file(mcp_path: &PathBuf, binary: &str, label: &str) {
    write_mcp_config(mcp_path, binary, label, "servers", server_entry(binary));
}

fn write_mcp_config(
    mcp_path: &PathBuf,
    binary: &str,
    label: &str,
    root_key: &str,
    desired: serde_json::Value,
) {
    if mcp_path.exists() {
        let content = std::fs::read_to_string(mcp_path).unwrap_or_default();
        match crate::core::jsonc::parse_jsonc(&content) {
            Ok(mut json) => {
                if let Some(obj) = json.as_object_mut() {
                    let servers = obj.entry(root_key).or_insert_with(|| serde_json::json!({}));
                    if let Some(servers_obj) = servers.as_object_mut() {
                        if servers_obj.get("lean-ctx") == Some(&desired) {
                            if !mcp_server_quiet_mode() {
                                eprintln!(
                                    "  \x1b[32m✓\x1b[0m lean-ctx already configured in {label}"
                                );
                            }
                            return;
                        }
                        servers_obj.insert("lean-ctx".to_string(), desired);
                    }
                    write_file(
                        mcp_path,
                        &serde_json::to_string_pretty(&json).unwrap_or_default(),
                    );
                    if !mcp_server_quiet_mode() {
                        eprintln!("  \x1b[32m✓\x1b[0m Added lean-ctx to {label}");
                    }
                    return;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Could not parse MCP config at {}: {e}\nAdd to \"{root_key}\": \"lean-ctx\": {{ \"command\": \"{}\", \"args\": [] }}",
                    mcp_path.display(),
                    binary
                );
                return;
            }
        }
    }

    if let Some(parent) = mcp_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let config = serde_json::json!({
        root_key: {
            "lean-ctx": desired
        }
    });

    write_file(
        mcp_path,
        &serde_json::to_string_pretty(&config).unwrap_or_default(),
    );
    if !mcp_server_quiet_mode() {
        eprintln!("  \x1b[32m✓\x1b[0m Created {label} with lean-ctx MCP server");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_entry_carries_bash_and_powershell_with_quoting() {
        // Windows-style install path with a space — the #381 failure shape.
        let entry = copilot_hook_entry(
            r"C:\Users\Jane Doe\AppData\Local\lean-ctx.exe",
            "compress",
            15,
        );
        assert_eq!(
            entry["bash"], "\"/c/Users/Jane Doe/AppData/Local/lean-ctx.exe\" hook compress",
            "bash field must use a quoted MSYS-style path"
        );
        assert_eq!(
            entry["powershell"],
            "& \"C:\\Users\\Jane Doe\\AppData\\Local\\lean-ctx.exe\" hook compress",
            "powershell field must invoke the quoted native path via the call operator"
        );
        assert_eq!(entry["timeoutSec"], 15);
        assert_eq!(entry["type"], "command");

        // Unix paths stay untouched (modulo quoting).
        let unix = copilot_hook_entry("/usr/local/bin/lean-ctx", "observe", 5);
        assert_eq!(unix["bash"], "\"/usr/local/bin/lean-ctx\" hook observe");
        assert_eq!(
            unix["powershell"],
            "& \"/usr/local/bin/lean-ctx\" hook observe"
        );
    }
}
