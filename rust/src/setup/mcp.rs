//! Per-agent MCP configuration (configure/disable, target resolution).
//!
//! Split out of `setup/mod.rs`; `use super::*` re-imports the parent module’s
//! aliases and sibling helpers. Public fns are re-exported via `pub(crate) use`.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Result of setting up a single agent with all steps.
#[derive(Debug, Default)]
pub struct AgentSetupResult {
    pub mcp_ok: bool,
    pub rules: crate::rules_inject::InjectResult,
    pub errors: Vec<String>,
}

/// Complete per-agent setup: MCP config + global rules + hook.
/// Single source of truth — called by both `init --agent` and `setup`.
pub fn setup_single_agent(
    agent_name: &str,
    global: bool,
    mode: crate::hooks::HookMode,
) -> AgentSetupResult {
    // Prefer HOME/USERPROFILE so tests and portable installs work on Windows
    // (dirs::home_dir uses Known Folder API and ignores env overrides).
    let home = crate::core::home::resolve_home_dir().unwrap_or_default();
    let mut result = AgentSetupResult::default();

    crate::hooks::install_agent_hook_with_mode(agent_name, global, mode);

    match configure_agent_mcp(agent_name) {
        Ok(()) => result.mcp_ok = true,
        Err(e) => result.errors.push(format!("MCP config: {e}")),
    }

    result.rules = crate::rules_inject::inject_rules_for_agent(&home, agent_name);

    result
}

pub fn configure_agent_mcp(agent: &str) -> Result<(), String> {
    // Prefer HOME/USERPROFILE (dirs::home_dir ignores env on Windows).
    let home = crate::core::home::resolve_home_dir()
        .ok_or_else(|| "Cannot determine home directory".to_string())?;
    let binary = resolve_portable_binary();

    let targets = agent_mcp_targets(agent, &home)?;

    let mut errors = Vec::new();
    for t in &targets {
        if let Err(e) = crate::core::editor_registry::write_config_with_options(
            t,
            &binary,
            WriteOptions {
                overwrite_invalid: true,
            },
        ) {
            eprintln!(
                "\x1b[33m⚠\x1b[0m  Could not configure {}: {}",
                t.config_path.display(),
                e
            );
            errors.push(e);
        }
    }

    if (agent == "vscode" || agent == "copilot")
        && let Err(e) = crate::core::editor_registry::plan_mode::write_vscode_plan_settings()
    {
        eprintln!("\x1b[33m⚠\x1b[0m  VS Code plan mode: {e}");
    }
    if (agent == "claude" || agent == "claude-code")
        && let Err(e) =
            crate::core::editor_registry::plan_mode::write_claude_code_plan_permissions()
    {
        eprintln!("\x1b[33m⚠\x1b[0m  Claude Code plan mode: {e}");
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} config(s) could not be written. See warnings above.",
            errors.len()
        ))
    }
}

pub(crate) fn agent_mcp_targets(
    agent: &str,
    home: &std::path::Path,
) -> Result<Vec<EditorTarget>, String> {
    let mut targets = Vec::<EditorTarget>::new();

    let push = |targets: &mut Vec<EditorTarget>,
                name: &'static str,
                config_path: PathBuf,
                config_type: ConfigType| {
        targets.push(EditorTarget {
            name,
            agent_key: agent.to_string(),
            detect_path: PathBuf::from("/nonexistent"), // not used in direct agent config
            config_path,
            config_type,
        });
    };

    match agent {
        "cursor" => push(
            &mut targets,
            "Cursor",
            home.join(".cursor/mcp.json"),
            ConfigType::McpJson,
        ),
        "claude" | "claude-code" => push(
            &mut targets,
            "Claude Code",
            crate::core::editor_registry::claude_mcp_json_path(home),
            ConfigType::McpJson,
        ),
        "windsurf" => push(
            &mut targets,
            "Windsurf",
            home.join(".codeium/windsurf/mcp_config.json"),
            ConfigType::McpJson,
        ),
        "codex" => push(
            &mut targets,
            "Codex CLI",
            crate::core::home::resolve_codex_config_path()
                .unwrap_or_else(|| home.join(".codex/config.toml")),
            ConfigType::Codex,
        ),
        "gemini" => {
            push(
                &mut targets,
                "Gemini CLI",
                home.join(".gemini/settings.json"),
                ConfigType::GeminiSettings,
            );
        }
        "copilot" => push(
            &mut targets,
            "Copilot CLI",
            home.join(".copilot/mcp-config.json"),
            ConfigType::CopilotCli,
        ),
        "opencode" => {
            #[cfg(windows)]
            let opencode_path = if let Ok(appdata) = std::env::var("APPDATA") {
                std::path::PathBuf::from(appdata)
                    .join("opencode")
                    .join("opencode.json")
            } else {
                home.join(".config/opencode/opencode.json")
            };
            #[cfg(not(windows))]
            let opencode_path = home.join(".config/opencode/opencode.json");
            push(
                &mut targets,
                "OpenCode",
                opencode_path,
                ConfigType::OpenCode,
            );
        }
        "vscode" => push(
            &mut targets,
            "VS Code",
            crate::core::editor_registry::vscode_mcp_path(),
            ConfigType::VsCodeMcp,
        ),
        "zed" => push(
            &mut targets,
            "Zed",
            crate::core::editor_registry::zed_settings_path(home),
            ConfigType::Zed,
        ),
        _ => {
            return Err(format!("Unknown agent '{agent}'"));
        }
    }

    Ok(targets)
}
