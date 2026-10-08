//! The MCP server `instructions` — the one place sobre tells an agent how to
//! use it.
//!
//! sobre writes no rules into the agent's own files (CLAUDE.md, AGENTS.md,
//! `.cursor/rules`, …). Every client gets this same text on `initialize`, and
//! the only setting that changes it is `compression_level`: it picks the output
//! style appended below. The detailed guide is the `sobre`
//! skill, which agents that support skills load on demand.
//!
//! The text must stay true on every host: hooks compress native shell output
//! where the host supports them, and native tools are never denied, so nothing
//! here may claim they are.

use crate::core::config::CompressionLevel;

/// Claude Code truncates a server's instructions at 2048 characters; the
/// longest style (`Raw`) must fit under it.
const MAX_INSTRUCTION_CHARS: usize = 2048;

/// What every client is told, whatever its level.
pub const GUIDE: &str = "\
sobre: local context tools. Prefer them when they fit; native tools keep working.
• Orient: ctx_compose first (no task = project overview; with task = ranked files + source).
• Read: ctx_read(path, mode=auto|signatures|map|full|lines:N-M|anchored). Unchanged re-reads cost ~13 tokens.
• Find: ctx_search (regex | action=symbol | action=semantic), ctx_glob (files), ctx_tree, ctx_outline (symbols).
• Impact: ctx_graph (action=callers|callees|trace for calls; impact|changes|path for files).
• Edit: ctx_read(mode=anchored) → ctx_patch (line+hash anchors; op=create for new files). Native Edit also works.
• Shell: ctx_shell compresses output; long jobs → run_in_background=true.
• Memory: ctx_session (task, findings, decisions), ctx_knowledge (facts across sessions); undo: ctx_checkpoint.
RECOVER: compressed output is not exact evidence. Footer id → ctx_expand(id); file → ctx_read(mode=full) or read the shown path; shell → ctx_shell(raw=true) or `sobre raw \"<command>\"`. Never rebuild exact content line-by-line.
Full guide: the `sobre` skill, if your agent has skills.
OUTPUT: never echo tool output, no narration comments, show only changed code.";

/// Output-style prompt for a compression level (empty for `Off`).
pub fn compression_text(level: CompressionLevel) -> &'static str {
    match level {
        CompressionLevel::Off => "",
        CompressionLevel::Lite => {
            "OUTPUT STYLE: concise — bullets over paragraphs, no filler or hedging, \
             1-sentence explanations then code/action, never repeat the user."
        }
        CompressionLevel::Standard => {
            "OUTPUT STYLE: dense — one atomic fact per line; abbreviate fn, cfg, impl, deps, \
             req, res, ctx, err, ret; diff lines (+/-/~) only; → causes, ∴ therefore; \
             no narration; ≤200 tokens unless code."
        }
        CompressionLevel::Max => {
            "OUTPUT STYLE: expert-terse — telegraph format, drop articles; symbols → ∵ ∴ ⊕ ⊖ Δ ≈ ≠ ∈ ∅ ✓ ✗; \
             code blocks untouched; lines ≤80 chars; zero narration; ≤100 tokens per non-code reply."
        }
        CompressionLevel::Raw => {
            "OUTPUT STYLE: raw-dense — bullets only, no prose, intro or outro; +added -removed \
             ~changed !breaking; one fact per line ≤60 chars; symbols as expert-terse; code \
             untouched; ≤50 tokens per non-code reply; show results, never explain them."
        }
    }
}

/// The instructions for the configured `compression_level`. A session's
/// temporary lowering of compression changes tool output only, never this
/// text (invariant 3: one tools/list, one set of instructions).
#[must_use]
pub fn build_instructions() -> String {
    render(CompressionLevel::effective_configured(
        &crate::core::config::Config::load(),
    ))
}

/// The instructions for an explicit level.
#[must_use]
pub fn render(level: CompressionLevel) -> String {
    let mut parts = vec![GUIDE.to_string()];
    let shell = shell_hint();
    if !shell.is_empty() {
        parts.push(shell);
    }
    let style = compression_text(level);
    if !style.is_empty() {
        parts.push(style.to_string());
    }
    let out = parts.join("\n");
    if out.len() <= MAX_INSTRUCTION_CHARS {
        out
    } else {
        out[..out.floor_char_boundary(MAX_INSTRUCTION_CHARS)].to_string()
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

    const LEVELS: [CompressionLevel; 5] = [
        CompressionLevel::Off,
        CompressionLevel::Lite,
        CompressionLevel::Standard,
        CompressionLevel::Max,
        CompressionLevel::Raw,
    ];
    #[test]
    fn every_level_fits_claude_codes_limit_untruncated() {
        for level in LEVELS {
            let out = render(level);
            let full = GUIDE.len() + compression_text(level).len();
            assert!(
                full + 64 <= MAX_INSTRUCTION_CHARS,
                "{level:?}: {full} chars leaves no room under {MAX_INSTRUCTION_CHARS}"
            );
            assert!(out.contains(compression_text(level)));
        }
    }

    #[test]
    fn only_the_level_changes_the_text() {
        let off = render(CompressionLevel::Off);
        assert_eq!(off, GUIDE.to_string() + &shell_hint_line());
        let lite = render(CompressionLevel::Lite);
        assert!(lite.starts_with(GUIDE) && lite.contains("OUTPUT STYLE: concise"));
        // One writing-style line, no second prompt that repeats or contradicts it.
        for level in LEVELS {
            let text = render(level);
            assert!(!text.contains("CRP:"), "{level:?}");
            assert!(
                !text.to_lowercase().contains("trust tool outputs"),
                "{level:?}"
            );
        }
    }

    fn shell_hint_line() -> String {
        let h = shell_hint();
        if h.is_empty() { h } else { format!("\n{h}") }
    }

    #[test]
    fn every_named_tool_is_registered() {
        let named: std::collections::BTreeSet<&str> = GUIDE
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| w.starts_with("ctx_") && w.len() > 4)
            .collect();
        assert!(named.len() >= 13, "guide names {named:?}");
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
    }

    #[test]
    fn teaches_every_way_back_to_exact_content() {
        // The recovery paths agents cannot discover on their own once output is
        // compressed: the expand id, a full read, and raw shell output — the
        // last one on the CLI too, since hooks compress native shell output.
        for path in [
            "ctx_expand(id)",
            "ctx_read(mode=full)",
            "ctx_shell(raw=true)",
            "sobre raw \"<command>\"",
            "not exact evidence",
        ] {
            assert!(GUIDE.contains(path), "guide must name {path}");
        }
    }
}
