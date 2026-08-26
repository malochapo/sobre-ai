//! Rules section builders (#756).
//!
//! Each function generates the text for one rules section. Tool references
//! are gated on whether the tool is registered, so a name that left the
//! registry stops being taught to the agent. This used to be gated on the
//! active profile's tier instead; the tiers no longer select anything.
//!
//! Folded-tool alignment (#509): `ctx_symbol` → `ctx_search(action=symbol)`,
//! `ctx_semantic_search` → `ctx_search(action=semantic)`.

fn has(name: &str) -> bool {
    crate::server::registry::is_registered(name)
}

/// Intent-to-tool playbook — only advertises tools the profile exposes.
pub fn intent_section() -> String {
    let mut lines = vec!["Tool selection by intent:".to_string()];

    if has("ctx_compose") {
        lines.push("• Orient / understand code (call FIRST) -> ctx_compose".into());
    }

    let read_line = if has("ctx_patch") {
        "• Read a file -> ctx_read(path, mode=signatures|map|full); edit after reading -> ctx_patch"
    } else {
        "• Read a file -> ctx_read(path, mode=signatures|map|full)"
    };
    lines.push(read_line.into());

    // #509: ctx_symbol and ctx_semantic_search are folded into ctx_search actions.
    let mut search_parts = vec!["• Exact symbol -> ctx_search(action=symbol)"];
    search_parts.push("pattern -> ctx_search");
    search_parts.push("by meaning -> ctx_search(action=semantic)");
    lines.push(search_parts.join("; "));

    let mut nav = String::from("• Files by glob -> ctx_glob; structure -> ctx_tree");
    if has("ctx_callgraph") {
        nav.push_str("; callers/impact -> ctx_callgraph");
    }
    lines.push(nav);

    let mut verify = String::from("• Verify after edits -> ctx_shell(test/build)");
    if has("ctx_session") || has("ctx_knowledge") {
        verify.push_str("; memory -> ctx_session / ctx_knowledge");
    }
    lines.push(verify);

    lines.push(
        "Semantic questions -> search tools, not whole-file reads: \
         reading more ≠ understanding more."
            .into(),
    );
    lines.join("\n")
}

/// Mandatory tool mapping on hook-covered hosts (Cursor with hooks).
/// v8: strict mode — always prefer ctx_* even when hooks would cover native tools.
pub fn hook_covered_tools_section() -> String {
    let mut lines = vec!["MANDATORY MAPPING (always use ctx_* instead of native):".to_string()];

    lines.push("• Read/cat -> ctx_read(path, mode) — cached, 10 modes, re-reads ~13 tokens".into());
    lines.push(
        "• Grep/search -> ctx_search(pattern, path) — also action=symbol|semantic \
         for definitions/meaning"
            .into(),
    );
    lines.push("• Shell/bash -> ctx_shell(command) — 95+ compression patterns".into());

    if has("ctx_compose") {
        lines.push(
            "• ctx_compose — orient in code FIRST (bundles search + read + symbols) \
             — call before editing/debugging"
                .into(),
        );
    }

    if has("ctx_callgraph") {
        lines.push(
            "• ctx_callgraph — callers, callees, blast radius — use instead of manual \
             file reading"
                .into(),
        );
    }

    if has("ctx_session") || has("ctx_knowledge") {
        lines.push(
            "• ctx_session / ctx_knowledge — persistent memory — record decisions & \
             progress after milestones"
                .into(),
        );
    }

    if has("ctx_expand") {
        lines.push("• ctx_expand — recover full text from [Archived]/compressed output".into());
    }

    lines.join("\n")
}

/// Shadow-mode exclusive tools (no native trigger to intercept).
pub fn shadow_minimal_section() -> String {
    let mut exclusives = Vec::new();

    if has("ctx_compose") {
        exclusives.push("ctx_compose (understand code, call first)");
    }

    // #509: always ctx_search actions, not standalone tools
    exclusives.push("ctx_search(action=symbol) (exact symbol)");
    exclusives.push("ctx_search(action=semantic) (by meaning)");

    if has("ctx_callgraph") {
        exclusives.push("ctx_callgraph (callers)");
    }
    if has("ctx_knowledge") || has("ctx_session") {
        exclusives.push("ctx_knowledge / ctx_session (memory)");
    }

    let edit_line = if has("ctx_patch") {
        "File editing → native Edit/StrReplace (lean-ctx only handles reads); if denied, use ctx_patch.\n"
    } else {
        "File editing → native Edit/StrReplace (lean-ctx only handles reads).\n"
    };

    format!(
        "lean-ctx shadow mode: native read/search/shell calls auto-route to ctx_* \
         — no tool-mapping needed.\n\
         {edit_line}\
         Exclusive tools (no native trigger): {}.",
        exclusives.join(", ")
    )
}

/// Anti-patterns — only references tools the profile exposes.
pub fn anti_section() -> String {
    let mut lines = vec!["Anti-patterns — do NOT:".to_string()];

    if has("ctx_compose") {
        lines.push(
            "• Chain ctx_search -> ctx_read -> ctx_search(action=symbol) \
             — one ctx_compose replaces all three"
                .into(),
        );
    }

    lines.push("• Use ctx_read(mode=full) for orientation — use mode=signatures".into());

    if has("ctx_callgraph") || has("ctx_graph") {
        lines.push(
            "• Use ctx_callgraph/ctx_graph for const/static/variable refs — they track \
             call edges and file deps only; use ctx_search instead"
                .into(),
        );
    }

    lines.join("\n")
}

/// LITM end-of-instructions preference line — only lists enabled tools.
pub fn litm_end_section() -> String {
    let mut prefs = Vec::new();

    if has("ctx_compose") {
        prefs.push("ctx_compose>chain");
    }
    prefs.push("ctx_read>Read");
    prefs.push("ctx_shell>Shell");
    prefs.push("ctx_search>Grep");
    prefs.push("ctx_glob>Glob");
    prefs.push("ctx_tree>ls");

    format!(
        "TOOL PREFERENCE (END): {} | Edit/Write/Delete=native",
        prefs.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_names_the_registered_toolbox() {
        let text = intent_section();
        for tool in [
            "ctx_read",
            "ctx_search",
            "ctx_compose",
            "ctx_callgraph",
            "ctx_patch",
        ] {
            assert!(
                !crate::server::registry::is_registered(tool) || text.contains(tool),
                "intent section must name the registered {tool}"
            );
        }
    }

    /// A tool that left the registry must not be taught. `ctx_symbol` and
    /// `ctx_semantic_search` were folded into `ctx_search` actions (#509), so
    /// they are the standing case.
    #[test]
    fn unregistered_tools_are_never_named() {
        for tool in ["ctx_symbol", "ctx_semantic_search", "ctx_multi_read"] {
            assert!(
                !crate::server::registry::is_registered(tool),
                "test premise broken: {tool} is registered again"
            );
        }
        for text in [
            intent_section(),
            hook_covered_tools_section(),
            shadow_minimal_section(),
            anti_section(),
            litm_end_section(),
        ] {
            for tool in ["ctx_symbol", "ctx_semantic_search", "ctx_multi_read"] {
                // `ctx_search(action=symbol)` legitimately contains "symbol",
                // so match the bare tool name followed by a word boundary.
                for hit in text.match_indices(tool) {
                    let after = text[hit.0 + tool.len()..].chars().next();
                    assert!(
                        after.is_some_and(|c| c.is_alphanumeric() || c == '_'),
                        "section names the unregistered {tool}:
{text}"
                    );
                }
            }
        }
    }

    #[test]
    fn intent_uses_folded_search_actions() {
        let text = intent_section();
        assert!(text.contains("ctx_search(action=symbol)"));
        assert!(text.contains("ctx_search(action=semantic)"));
    }

    #[test]
    fn hook_covered_section_maps_the_native_tools() {
        let text = hook_covered_tools_section();
        for tool in ["ctx_read", "ctx_search", "ctx_shell"] {
            assert!(text.contains(tool), "hook-covered mapping must name {tool}");
        }
    }

    #[test]
    fn anti_section_keeps_the_universal_pattern() {
        let text = anti_section();
        assert!(text.contains("ctx_read(mode=full)"));
    }

    #[test]
    fn litm_end_always_has_core_tools() {
        let text = litm_end_section();
        assert!(text.contains("ctx_read>Read"));
        assert!(text.contains("ctx_shell>Shell"));
        assert!(text.contains("ctx_search>Grep"));
    }

    #[test]
    fn shadow_section_lists_the_exclusive_tools() {
        let text = shadow_minimal_section();
        assert!(text.contains("shadow mode"), "{text}");
        assert!(text.contains("ctx_search(action=symbol)"), "{text}");
    }
}
