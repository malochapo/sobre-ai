//! Per-client instruction delivery (Claude Code 2048-char MCP cap and its
//! CLAUDE.md block layout).

#[allow(clippy::wildcard_imports)]
use crate::doctor::common::*;
use crate::doctor::{BOLD, DIM, GREEN, Outcome, RST, YELLOW};

pub(crate) fn claude_truncation_outcome() -> Option<Outcome> {
    let home = dirs::home_dir()?;
    let claude_detected = crate::core::editor_registry::claude_mcp_json_path(&home).exists()
        || crate::core::editor_registry::claude_state_dir(&home).exists()
        || claude_binary_exists();

    if !claude_detected {
        return None;
    }

    Some(claude_instructions_check(&home))
}
/// Verify Claude Code receives the full lean-ctx instructions despite the
/// 2048-char MCP instructions cap.
///
/// The v3 layout (GL #555) replaced the always-loaded `~/.claude/rules/lean-ctx.md`
/// with a CLAUDE.md block + on-demand skill — `setup` actively *removes* the rules
/// file. The check therefore accepts every layout `setup` can produce (GH #396:
/// the old check demanded the retired rules file right after setup deleted it,
/// and its suggested fix could not recreate one). Layout detection lives in
/// `common::claude_instructions_state`, shared with `doctor integrations`.
pub(crate) fn claude_instructions_check(home: &std::path::Path) -> Outcome {
    use crate::doctor::common::ClaudeInstructionsState as S;

    let state = crate::doctor::common::claude_instructions_state(home);
    let line = match state {
        S::Block => format!(
            "{BOLD}Claude Code instructions{RST}  {GREEN}CLAUDE.md block installed{RST}  {DIM}(MCP instructions capped at 2048 chars — full content via CLAUDE.md){RST}"
        ),
        S::Missing => format!(
            "{BOLD}Claude Code instructions{RST}  {YELLOW}no CLAUDE.md block or rules file found — MCP instructions truncated at 2048 chars{RST}  {DIM}(run: lean-ctx init){RST}"
        ),
    };
    Outcome {
        ok: state.ok(),
        line,
    }
}
