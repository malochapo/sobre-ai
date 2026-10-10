//! The one-line summary `sobre_read` keeps for a file it delivered.

/// Extracts a one-line structural hint from file/tool output.
pub fn extract_content_hint(output: &str) -> String {
    let lines: Vec<&str> = output.lines().skip(1).take(20).collect();

    // Layer 1: deps/exports/module-level descriptions
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("deps:")
            || trimmed.starts_with("exports:")
            || trimmed.starts_with("//!")
        {
            return trimmed[..trimmed.floor_char_boundary(80)].to_string();
        }
    }

    // Layer 2: primary struct/fn/class/trait definitions
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("pub struct ")
            || trimmed.starts_with("pub fn ")
            || trimmed.starts_with("pub enum ")
            || trimmed.starts_with("pub trait ")
            || trimmed.starts_with("impl ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("export ")
            || trimmed.starts_with("export default ")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("def ")
            || trimmed.starts_with("func ")
        {
            return trimmed[..trimmed.floor_char_boundary(70)].to_string();
        }
    }

    // Layer 3: doc comments / markdown headings
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("///") || trimmed.starts_with("# ") {
            return trimmed[..trimmed.floor_char_boundary(70)].to_string();
        }
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_content_hint_survives_multibyte_byte_budget() {
        // Regression: a matched line whose byte-budget cut (70/80) lands
        // inside a 2-byte Cyrillic char must snap to a char boundary, not panic.
        // `extract_content_hint` skips the first line, so each input has a header.
        // Layer 3 — markdown / doc heading (the reported auto_findings.rs:424,
        // budget 70). "# _" is 3 bytes, so byte 70 lands mid-char in the run.
        let line = format!("# _{}", "я".repeat(40));
        let hint = extract_content_hint(&format!("header\n{line}"));
        assert!(
            line.starts_with(&hint),
            "hint must be a valid prefix: {hint}"
        );
        assert!(hint.starts_with("# _") && (60..=70).contains(&hint.len()));

        // Layer 2 — definition line (budget 70).
        let def = format!("def _{}", "я".repeat(40));
        let hint = extract_content_hint(&format!("header\n{def}"));
        assert!(def.starts_with(&hint) && hint.starts_with("def _") && hint.len() <= 70);

        // Layer 1 — module doc comment (budget 80).
        let doc = format!("//!{}", "я".repeat(40));
        let hint = extract_content_hint(&format!("header\n{doc}"));
        assert!(doc.starts_with(&hint) && hint.starts_with("//!") && hint.len() <= 80);
    }
}
