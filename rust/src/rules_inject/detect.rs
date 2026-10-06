//! Tool detection: is the agent actually installed on this machine?

use std::path::PathBuf;

use super::RulesTarget;

pub(super) fn is_tool_detected(target: &RulesTarget, home: &std::path::Path) -> bool {
    match target.name {
        "Claude Code" => {
            if command_exists("claude") {
                return true;
            }
            let state_dir = crate::core::editor_registry::claude_state_dir(home);
            crate::core::editor_registry::claude_mcp_json_path(home).exists() || state_dir.exists()
        }
        "Codex CLI" => {
            let codex_dir =
                crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
            codex_dir.exists() || command_exists("codex")
        }
        "Cursor" => home.join(".cursor").exists(),
        "Windsurf" => home.join(".codeium/windsurf").exists(),
        "Gemini CLI" => home.join(".gemini").exists(),
        "VS Code" => detect_vscode_installed(home),
        "Copilot CLI" => home.join(".copilot").exists() || command_exists("copilot"),
        "Zed" => crate::core::editor_registry::zed_config_dir(home).exists(),
        "OpenCode" => home.join(".config/opencode").exists(),
        _ => false,
    }
}

pub(super) fn command_exists(name: &str) -> bool {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("where")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success());

    #[cfg(not(target_os = "windows"))]
    let result = std::process::Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success());

    result
}

fn detect_vscode_installed(_home: &std::path::Path) -> bool {
    let check_dir = |dir: PathBuf| -> bool {
        dir.join("settings.json").exists() || dir.join("mcp.json").exists()
    };

    #[cfg(target_os = "macos")]
    if check_dir(_home.join("Library/Application Support/Code/User")) {
        return true;
    }
    #[cfg(target_os = "linux")]
    if check_dir(_home.join(".config/Code/User")) {
        return true;
    }
    if check_dir(_home.join(".config/Code - Insiders/User")) {
        return true;
    }
    if check_dir(_home.join(".vscode-server/data/User")) {
        return true;
    }
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        if check_dir(PathBuf::from(&appdata).join("Code/User")) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Target definitions
// ---------------------------------------------------------------------------
