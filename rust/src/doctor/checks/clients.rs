//! Per-client guidance delivery: the `sobre` skill for Claude Code.

#[allow(clippy::wildcard_imports)]
use crate::doctor::common::*;
use crate::doctor::{BOLD, DIM, GREEN, Outcome, RST, YELLOW};

/// The skill check, when Claude Code is installed.
pub(crate) fn claude_skill_outcome() -> Option<Outcome> {
    let home = dirs::home_dir()?;
    let claude_detected = crate::core::editor_registry::claude_mcp_json_path(&home).exists()
        || crate::core::editor_registry::claude_state_dir(&home).exists()
        || claude_binary_exists();

    if !claude_detected {
        return None;
    }

    Some(claude_skill_check(&home))
}

/// Verify Claude Code has the current `sobre` skill — the detailed guide the
/// MCP instructions point to.
pub(crate) fn claude_skill_check(home: &std::path::Path) -> Outcome {
    let current = crate::setup::skill::is_current("claude", home);
    let line = if current {
        format!("{BOLD}Claude Code skill{RST}  {GREEN}sobre skill installed{RST}")
    } else {
        format!(
            "{BOLD}Claude Code skill{RST}  {YELLOW}sobre skill missing or outdated{RST}  {DIM}(run: sobre init){RST}"
        )
    };
    Outcome { ok: current, line }
}
