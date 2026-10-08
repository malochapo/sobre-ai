//! The `sobre` agent skill: the detailed usage guide, loaded on demand by
//! agents that support skills. The MCP instructions carry the short version
//! to every client; this file is never auto-loaded, so it costs nothing until
//! an agent opens it.
//!
//! One file, written whole into `<agent skills dir>/sobre/SKILL.md` and owned
//! entirely by sobre, so uninstall removes the directory.

use std::path::{Path, PathBuf};

pub const SKILL_MD: &str = include_str!("../../skill/SKILL.md");

const SKILL_NAME: &str = "sobre";

/// Skills directory of each agent that loads `SKILL.md` skills. Agents not
/// listed (Gemini CLI, Windsurf, Zed) rely on the MCP instructions alone.
fn skills_dir(agent: &str, home: &Path) -> Option<PathBuf> {
    match agent {
        "claude" | "claude-code" => {
            Some(crate::core::editor_registry::claude_state_dir(home).join("skills"))
        }
        "codex" => Some(
            crate::core::home::resolve_codex_dir()
                .unwrap_or_else(|| home.join(".codex"))
                .join("skills"),
        ),
        "cursor" => Some(home.join(".cursor/skills")),
        "copilot" | "vscode" => Some(home.join(".copilot/skills")),
        "opencode" => Some(home.join(".config/opencode/skills")),
        _ => None,
    }
}

/// Where the skill lives for `agent`, if that agent supports skills.
pub fn skill_path(agent: &str, home: &Path) -> Option<PathBuf> {
    skills_dir(agent, home).map(|d| d.join(SKILL_NAME).join("SKILL.md"))
}

/// True when `agent` has the current skill installed.
pub fn is_current(agent: &str, home: &Path) -> bool {
    skill_path(agent, home)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|c| c == SKILL_MD)
}

/// Write the skill for `agent`. `Ok(None)` when the agent has no skills.
pub fn install(agent: &str, home: &Path) -> Result<Option<PathBuf>, String> {
    let Some(path) = skill_path(agent, home) else {
        return Ok(None);
    };
    if is_current(agent, home) {
        return Ok(Some(path));
    }
    if let Some(dir) = path.parent() {
        crate::config_io::ensure_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    crate::config_io::write_owned(&path, SKILL_MD)?;
    Ok(Some(path))
}

/// Every skill directory sobre may have written, for uninstall.
pub fn installed_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["claude", "codex", "cursor", "copilot", "opencode"]
        .into_iter()
        .filter_map(|a| skills_dir(a, home))
        .map(|d| d.join(SKILL_NAME))
        .filter(|d| d.join("SKILL.md").exists())
        .collect();
    dirs.dedup();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_the_frontmatter_agents_require() {
        let front = SKILL_MD
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .map(|(f, _)| f)
            .expect("SKILL.md starts with a YAML frontmatter block");
        assert!(front.lines().any(|l| l == "name: sobre"));
        let desc = front
            .lines()
            .find_map(|l| l.strip_prefix("description: "))
            .expect("description line");
        // Agents pick a skill from its description alone; keep it specific
        // and under the 1024-character limit skills loaders enforce.
        assert!(
            desc.len() > 100 && desc.len() <= 1024,
            "{} chars",
            desc.len()
        );
    }

    #[test]
    fn every_tool_the_skill_names_is_registered() {
        let named: std::collections::BTreeSet<&str> = SKILL_MD
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| w.starts_with("ctx_") && w.len() > 4)
            .collect();
        for tool in named {
            assert!(
                crate::server::registry::is_registered(tool),
                "SKILL.md names {tool}, which is not a registered tool"
            );
        }
    }

    #[test]
    fn install_is_idempotent_and_uninstall_finds_it() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let codex = home.join(".codex");
        let claude = home.join(".claude");
        let _codex = crate::setup::EnvVarGuard::set("CODEX_HOME", codex.to_str().unwrap());
        let _claude = crate::setup::EnvVarGuard::set("CLAUDE_CONFIG_DIR", claude.to_str().unwrap());

        let path = install("claude", home).unwrap().expect("claude has skills");
        assert_eq!(path, home.join(".claude/skills/sobre/SKILL.md"));
        assert!(is_current("claude", home));
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        install("claude", home).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);

        assert_eq!(install("zed", home).unwrap(), None);
        assert_eq!(
            installed_dirs(home),
            vec![home.join(".claude/skills/sobre")]
        );
    }
}
