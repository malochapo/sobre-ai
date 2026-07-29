//! Small, self-contained helper functions extracted from mod.rs to keep
//! the main module below the LOC gate.

pub fn is_instruction_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = std::path::Path::new(&lower)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");

    matches!(
        filename,
        "skill.md"
            | "agents.md"
            | "rules.md"
            | ".cursorrules"
            | ".clinerules"
            | "lean-ctx.md"
            | "lean-ctx.mdc"
    ) || lower.contains("/skills/")
        || lower.contains("/.cursor/rules/")
        || lower.contains("/.claude/rules/")
        || lower.contains("/agents.md")
}

pub(super) fn detect_project_root(path: &str) -> String {
    crate::core::protocol::detect_project_root_or_cwd(path)
}

/// Build graph-related hints (callers/callees) — exported for the registered
/// handler to call in a background thread after releasing the cache lock (#1098).
pub fn graph_related_hint(path: &str) -> Option<String> {
    let project_root = detect_project_root(path);
    crate::core::graph_context::build_related_hint(path, &project_root, 5)
}
