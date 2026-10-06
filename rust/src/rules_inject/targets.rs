//! The rules-target catalog: every supported agent and where its rules live.

use std::path::PathBuf;

use super::{RulesFormat, RulesTarget};

pub(super) fn build_rules_targets(home: &std::path::Path) -> Vec<RulesTarget> {
    // NOTE: Claude Code intentionally has NO rules target. Claude loads every
    // rules file without `paths:` frontmatter unconditionally at session start,
    // which duplicated the CLAUDE.md block in every session (12k+ token memory
    // footprints, GL #555). Claude guidance lives in the CLAUDE.md block
    // (hooks/agents/claude.rs) + the on-demand skill.
    vec![
        // --- Shared config files (append-only) ---
        RulesTarget {
            name: "Gemini CLI",
            path: home.join(".gemini/GEMINI.md"),
            format: RulesFormat::SharedMarkdown,
        },
        RulesTarget {
            name: "VS Code",
            path: copilot_instructions_path(home),
            format: RulesFormat::SharedMarkdown,
        },
        RulesTarget {
            name: "Copilot CLI",
            path: home.join(".copilot/instructions.md"),
            format: RulesFormat::SharedMarkdown,
        },
        // --- Dedicated lean-ctx rule files ---
        RulesTarget {
            name: "Cursor",
            path: home.join(".cursor/rules/lean-ctx.mdc"),
            format: RulesFormat::CursorMdc,
        },
        RulesTarget {
            name: "Windsurf",
            path: home.join(".codeium/windsurf/rules/lean-ctx.md"),
            format: RulesFormat::DedicatedMarkdown,
        },
        RulesTarget {
            name: "Zed",
            // OS-aware: Zed's config dir is platform-specific (macOS uses
            // Application Support); keep rules co-located with the MCP config.
            path: crate::core::editor_registry::zed_config_dir(home).join("rules/lean-ctx.md"),
            format: RulesFormat::DedicatedMarkdown,
        },
        RulesTarget {
            name: "OpenCode",
            path: home.join(".config/opencode/AGENTS.md"),
            format: RulesFormat::SharedMarkdown,
        },
        RulesTarget {
            name: "Codex CLI",
            path: crate::core::home::resolve_codex_dir()
                .unwrap_or_else(|| home.join(".codex"))
                .join("instructions.md"),
            format: RulesFormat::SharedMarkdown,
        },
    ]
}

fn copilot_instructions_path(home: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return home.join("Library/Application Support/Code/User/github-copilot-instructions.md");
    }
    #[cfg(target_os = "linux")]
    {
        let user_dirs = [
            home.join(".config/Code/User"),
            home.join(".config/Code - Insiders/User"),
            home.join(".vscode-server/data/User"),
        ];
        let user_dir = user_dirs
            .iter()
            .find(|p| p.exists())
            .cloned()
            .unwrap_or_else(|| user_dirs[0].clone());
        return user_dir.join("github-copilot-instructions.md");
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("Code/User/github-copilot-instructions.md");
        }
    }
    #[allow(unreachable_code)]
    home.join(".config/Code/User/github-copilot-instructions.md")
}
