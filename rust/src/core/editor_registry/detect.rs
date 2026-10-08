use std::path::{Path, PathBuf};

use super::paths::{
    claude_mcp_json_path, opencode_config_path, vscode_mcp_path, zed_config_dir, zed_settings_path,
};
use super::types::{ConfigType, EditorTarget};

pub fn build_targets(home: &Path) -> Vec<EditorTarget> {
    let opencode_cfg = opencode_config_path(home);
    let opencode_detect = opencode_cfg
        .parent()
        .map_or_else(|| home.join(".config/opencode"), Path::to_path_buf);

    vec![
        EditorTarget {
            name: "Cursor",
            agent_key: "cursor".to_string(),
            config_path: home.join(".cursor/mcp.json"),
            detect_path: home.join(".cursor"),
            config_type: ConfigType::McpJson,
        },
        EditorTarget {
            name: "Claude Code",
            agent_key: "claude".to_string(),
            config_path: claude_mcp_json_path(home),
            detect_path: detect_claude_path(),
            config_type: ConfigType::McpJson,
        },
        EditorTarget {
            name: "Windsurf",
            agent_key: "windsurf".to_string(),
            config_path: home.join(".codeium/windsurf/mcp_config.json"),
            detect_path: home.join(".codeium/windsurf"),
            config_type: ConfigType::McpJson,
        },
        EditorTarget {
            name: "Codex CLI",
            agent_key: "codex".to_string(),
            config_path: crate::core::home::resolve_codex_config_path()
                .unwrap_or_else(|| home.join(".codex/config.toml")),
            detect_path: detect_codex_path(home),
            config_type: ConfigType::Codex,
        },
        EditorTarget {
            name: "Gemini CLI",
            agent_key: "gemini".to_string(),
            config_path: home.join(".gemini/settings.json"),
            detect_path: home.join(".gemini"),
            config_type: ConfigType::GeminiSettings,
        },
        EditorTarget {
            name: "Zed",
            agent_key: "zed".to_string(),
            config_path: zed_settings_path(home),
            detect_path: zed_config_dir(home),
            config_type: ConfigType::Zed,
        },
        EditorTarget {
            name: "VS Code",
            agent_key: "vscode".to_string(),
            config_path: vscode_mcp_path(),
            detect_path: detect_vscode_path(),
            config_type: ConfigType::VsCodeMcp,
        },
        EditorTarget {
            name: "Copilot CLI",
            agent_key: "copilot".to_string(),
            config_path: home.join(".copilot/mcp-config.json"),
            detect_path: home.join(".copilot"),
            config_type: ConfigType::CopilotCli,
        },
        EditorTarget {
            name: "OpenCode",
            agent_key: "opencode".to_string(),
            config_path: opencode_cfg,
            detect_path: opencode_detect,
            config_type: ConfigType::OpenCode,
        },
    ]
}

pub fn detect_claude_path() -> PathBuf {
    let which_cmd = if cfg!(windows) { "where" } else { "which" };
    if let Ok(output) = std::process::Command::new(which_cmd).arg("claude").output()
        && output.status.success()
    {
        return PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    }
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            let p = PathBuf::from(dir);
            if p.exists() {
                return p;
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let claude_json = claude_mcp_json_path(&home);
        if claude_json.exists() {
            return claude_json;
        }
        // State dir counts as installed: doctor's Claude checks and the skill
        // both key off `~/.claude/` existing. Without this fallback
        // a machine with only the state dir hits a dead loop — doctor says
        // "run: sobre init", but setup skips Claude (detect_path missing)
        // and the warning never clears.
        let state_dir = super::claude_state_dir(&home);
        if state_dir.exists() {
            return state_dir;
        }
    }
    PathBuf::from("/nonexistent")
}

pub fn detect_codex_path(home: &Path) -> PathBuf {
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    if codex_dir.exists() {
        return codex_dir;
    }
    let which_cmd = if cfg!(windows) { "where" } else { "which" };
    if let Ok(output) = std::process::Command::new(which_cmd).arg("codex").output()
        && output.status.success()
    {
        return codex_dir;
    }
    PathBuf::from("/nonexistent")
}

pub fn detect_vscode_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            let vscode = home.join("Library/Application Support/Code/User/settings.json");
            if vscode.exists() {
                return vscode;
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = dirs::home_dir() {
            let paths = [
                home.join(".config/Code/User/settings.json"),
                home.join(".config/Code - Insiders/User/settings.json"),
                home.join(".vscode-server/data/User/settings.json"),
            ];
            for vscode in paths {
                if vscode.exists() {
                    return vscode;
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let vscode = PathBuf::from(appdata).join("Code/User/settings.json");
            if vscode.exists() {
                return vscode;
            }
        }
    }
    let which_cmd = if cfg!(windows) { "where" } else { "which" };
    if let Ok(output) = std::process::Command::new(which_cmd).arg("code").output()
        && output.status.success()
    {
        return PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    }
    PathBuf::from("/nonexistent")
}
