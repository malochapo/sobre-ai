//! The MCP server `instructions` — the one place sobre tells an agent how to
//! use it.
//!
//! sobre writes no rules into the agent's own files (CLAUDE.md, AGENTS.md,
//! `.cursor/rules`, …). Every client gets this same text on `initialize`,
//! whatever the configuration. Hosts put it in front of every session, so it
//! only says what the tools are and where the guide is: each tool describes
//! itself, every compressed output names its own way back to the full
//! content, and the detailed guide is the `sobre` skill, which agents that
//! support skills load on demand.
//!
//! The text must stay true on every host: hooks compress native shell output
//! where the host supports them, and native tools are never denied, so nothing
//! here may claim they are.

/// What every client is told.
pub const GUIDE: &str = "\
sobre: local tools to read, search, edit and run commands with less output; native tools keep working. \
sobre_compose gives a project overview, or ranked files with their source for a task. \
Full guide: the `sobre` skill, if your agent has skills.";

/// The instructions, the same for every client and every configuration
/// (invariant 3: one tools/list, one set of instructions). Only the shell
/// name is added on Windows, where commands may run under PowerShell.
#[must_use]
pub fn build_instructions() -> String {
    let shell = shell_hint();
    if shell.is_empty() {
        GUIDE.to_string()
    } else {
        format!("{GUIDE}\n{shell}")
    }
}

fn shell_hint() -> String {
    if !cfg!(windows) {
        return String::new();
    }
    let name = crate::shell::shell_name();
    if matches!(name.as_str(), "bash" | "sh" | "zsh" | "fish") {
        format!("SHELL: {name} (POSIX) — no PowerShell cmdlets.")
    } else if name.contains("powershell") || name.contains("pwsh") {
        format!("SHELL: {name}. Use PowerShell cmdlets.")
    } else {
        format!("SHELL: {name}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hosts send the instructions at the start of every session, before the
    /// agent asked for anything: they stay a pointer, not a manual.
    #[test]
    fn the_instructions_stay_short() {
        let text = build_instructions();
        assert!(GUIDE.len() <= 300, "{} chars", GUIDE.len());
        assert!(text.len() <= 360, "{} chars", text.len());
        assert!(text.starts_with(GUIDE));
    }

    /// No line asks the agent to change how it writes, and nothing sends it
    /// back to content it already has: every compressed output names its own
    /// way back.
    #[test]
    fn no_output_style_and_no_recovery_drill() {
        let lower = build_instructions().to_lowercase();
        for forbidden in ["output style", "output:", "recover", "re-read", "narration"] {
            assert!(!lower.contains(forbidden), "instructions say {forbidden:?}");
        }
    }

    #[test]
    fn every_named_tool_is_registered() {
        let named: std::collections::BTreeSet<&str> = GUIDE
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| w.starts_with("sobre_") && w.len() > 6)
            .collect();
        assert!(!named.is_empty(), "the guide names no tool");
        for tool in named {
            assert!(
                crate::server::registry::is_registered(tool),
                "the guide names {tool}, which is not a registered tool"
            );
        }
    }

    #[test]
    fn never_claims_native_tools_are_denied() {
        // Hooks compress native calls where the host supports them; nothing
        // sobre installs denies them, so the text must not say otherwise.
        let lower = GUIDE.to_lowercase();
        for forbidden in [
            "never use native",
            "denied",
            "replace mode",
            "claude.md",
            "agents.md",
        ] {
            assert!(!lower.contains(forbidden), "guide says {forbidden:?}");
        }
        assert!(lower.contains("native tools keep working"));
        assert!(GUIDE.contains("the `sobre` skill"));
    }
}
