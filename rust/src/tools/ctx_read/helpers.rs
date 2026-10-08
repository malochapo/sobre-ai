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
        "skill.md" | "agents.md" | "rules.md" | ".cursorrules"
    ) || (lower.contains("/skills/")
        && std::path::Path::new(&lower)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md")))
        || lower.contains("/.cursor/rules/")
        || lower.contains("/.claude/rules/")
        || lower.contains("/agents.md")
}
