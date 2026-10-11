//! Per-editor integration status builders (Cursor/Claude + the
//! generic registry-driven path).

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn integration_generic(
    home: &std::path::Path,
    binary: &str,
    target: &crate::core::editor_registry::types::EditorTarget,
) -> IntegrationStatus {
    let detected = target.detect_path.exists() || target.config_path.exists();
    if !detected {
        return IntegrationStatus {
            name: target.name.to_string(),
            detected: false,
            checks: Vec::new(),
            ok: true,
        };
    }

    let mut checks = Vec::new();
    match target.config_type {
        crate::core::editor_registry::types::ConfigType::McpJson => {
            checks.push(check_mcp_json(&target.config_path, binary));
        }
        crate::core::editor_registry::types::ConfigType::Zed => {
            checks.push(check_zed_settings(&target.config_path, binary));
        }
        crate::core::editor_registry::types::ConfigType::Codex => {
            checks.push(check_codex_toml(&target.config_path, binary));
            checks.push(check_codex_hooks_enabled(home));
            checks.push(check_codex_hooks_json(home, binary));
            checks.push(codex_desktop_note());
        }
        crate::core::editor_registry::types::ConfigType::VsCodeMcp => {
            checks.push(check_vscode_mcp(&target.config_path, binary));
        }
        crate::core::editor_registry::types::ConfigType::CopilotCli => {
            checks.push(check_copilot_cli_mcp(&target.config_path, binary));
        }
        crate::core::editor_registry::types::ConfigType::OpenCode => {
            checks.push(check_opencode_config(&target.config_path, binary));
        }
        crate::core::editor_registry::types::ConfigType::GeminiSettings => {
            checks.push(check_mcp_json(&target.config_path, binary));
            checks.push(check_gemini_hooks(home, binary));
        }
    }

    // Windsurf is wired through MCP + Cascade hooks and has no skills. Say so,
    // so a "missing skill" is not misread as a broken install, and show the
    // last sobre_* MCP call, so users can see whether Cascade actually drives
    // sobre (weaker models often call native tools instead).
    if target.name == "Windsurf" {
        checks.push(check_windsurf_hooks(home));
        checks.push(skill_not_applicable_note());
        checks.push(last_sobre_call_check());
    }

    let ok = checks.iter().all(|c| c.ok);
    IntegrationStatus {
        name: target.name.to_string(),
        detected: true,
        checks,
        ok,
    }
}

pub(crate) fn integration_cursor(home: &std::path::Path, binary: &str) -> IntegrationStatus {
    let cursor_dir = home.join(".cursor");
    if !cursor_dir.exists() {
        return IntegrationStatus {
            name: "Cursor".to_string(),
            detected: false,
            checks: Vec::new(),
            ok: true,
        };
    }

    let mut checks = Vec::new();
    let mcp_path = cursor_dir.join("mcp.json");
    checks.push(check_mcp_json(&mcp_path, binary));

    let hooks_path = cursor_dir.join("hooks.json");
    checks.push(check_cursor_hooks(&hooks_path, binary));

    let ok = checks.iter().all(|c| c.ok);
    IntegrationStatus {
        name: "Cursor".to_string(),
        detected: true,
        checks,
        ok,
    }
}

pub(crate) fn integration_claude(home: &std::path::Path, binary: &str) -> IntegrationStatus {
    let target = crate::core::editor_registry::build_targets(home)
        .into_iter()
        .find(|t| t.agent_key == "claude");
    let detected = target.as_ref().is_some_and(|t| t.detect_path.exists())
        || crate::core::editor_registry::claude_state_dir(home).exists()
        || claude_binary_exists();

    if !detected {
        return IntegrationStatus {
            name: "Claude Code".to_string(),
            detected: false,
            checks: Vec::new(),
            ok: true,
        };
    }

    let mut checks = Vec::new();
    let mcp_path = crate::core::editor_registry::claude_mcp_json_path(home);
    checks.push(check_mcp_json(&mcp_path, binary));

    let settings_path = crate::core::editor_registry::claude_state_dir(home).join("settings.json");
    checks.push(check_claude_hooks(&settings_path, binary));

    // The detailed guide to the MCP tools is the `sobre` skill, connected
    // with them; the MCP instructions carry the short version.
    if std::fs::read_to_string(&mcp_path).is_ok_and(|c| has_sobre_mcp_entry(&c)) {
        let current = crate::setup::skill::is_current("claude", home);
        let detail = match crate::setup::skill::skill_path("claude", home) {
            Some(p) if current => p.display().to_string(),
            _ => "missing or outdated (run: sobre init)".into(),
        };
        checks.push(NamedCheck {
            name: "Skill".to_string(),
            ok: current,
            detail,
        });
    }

    // What happens to Claude Code's own Read, so a user understands why native
    // reads are not compressed. Purely informational.
    {
        let detail = "native Read is not compressed; a re-read of an unchanged file comes back as a short stub (read-dedup hook); sobre_read compresses when the agent uses it";
        checks.push(NamedCheck {
            name: "Native Read".to_string(),
            ok: true,
            detail: detail.to_string(),
        });
    }

    let ok = checks.iter().all(|c| c.ok);
    IntegrationStatus {
        name: "Claude Code".to_string(),
        detected: true,
        checks,
        ok,
    }
}
