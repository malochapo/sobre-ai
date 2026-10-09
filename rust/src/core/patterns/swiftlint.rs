//! SwiftLint output compression.
//!
//! SwiftLint emits a `Linting 'File' (i/n)` progress line per file and one
//! `path:line:col: severity: Message (rule_id)` line per violation. The
//! progress goes; every violation and the final `Done linting!` total stay.
//! SwiftLint exits 0 when it finds only warnings, so this output reaches the
//! agent as a success and must still carry each warning's location.

use crate::core::compressor::strip_ansi;

macro_rules! static_regex {
    ($pattern:expr_2021) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| {
            regex::Regex::new($pattern).expect(concat!("BUG: invalid static regex: ", $pattern))
        })
    }};
}

fn violation_re() -> &'static regex::Regex {
    static_regex!(r"^(.+?):\d+:\d+:\s+(error|warning):\s+.*\(([a-z_][a-z0-9_]*)\)\s*$")
}

pub fn compress(_cmd: &str, output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Some("swiftlint: ok".to_string());
    }

    let mut kept = Vec::new();
    let mut violations = 0usize;
    for raw in trimmed.lines() {
        let stripped = strip_ansi(raw);
        let line = stripped.trim();
        if violation_re().is_match(line) {
            violations += 1;
            kept.push(line.to_string());
        } else if line.starts_with("Done linting!") {
            kept.push(line.to_string());
        }
    }

    if violations == 0 {
        if trimmed.contains("Found 0 violations") || trimmed.contains("0 violations") {
            return Some("swiftlint: clean".to_string());
        }
        return None;
    }
    Some(kept.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINT: &str = "Linting Swift files in current working directory\nLinting 'A.swift' (1/3)\nLinting 'B.swift' (2/3)\nLinting 'C.swift' (3/3)\n/path/A.swift:10:5: warning: Line Length Violation: Line should be 120 chars or less (line_length)\n/path/A.swift:22:1: warning: Trailing Whitespace Violation: no trailing whitespace (trailing_whitespace)\n/path/B.swift:5:1: error: Force Cast Violation: avoid force casts (force_cast)\n/path/A.swift:30:5: warning: Line Length Violation: too long (line_length)\nDone linting! Found 4 violations, 1 serious in 3 files.";

    #[test]
    fn keeps_every_violation_and_the_total_and_drops_progress() {
        let r = compress("swiftlint", LINT).unwrap();
        assert_eq!(
            r,
            "/path/A.swift:10:5: warning: Line Length Violation: Line should be 120 chars or less (line_length)\n\
             /path/A.swift:22:1: warning: Trailing Whitespace Violation: no trailing whitespace (trailing_whitespace)\n\
             /path/B.swift:5:1: error: Force Cast Violation: avoid force casts (force_cast)\n\
             /path/A.swift:30:5: warning: Line Length Violation: too long (line_length)\n\
             Done linting! Found 4 violations, 1 serious in 3 files."
        );
    }

    #[test]
    fn clean_run() {
        let r = compress(
            "swiftlint",
            "Linting 'A.swift' (1/1)\nDone linting! Found 0 violations, 0 serious in 1 file.",
        )
        .unwrap();
        assert_eq!(r, "swiftlint: clean");
    }

    #[test]
    fn empty_is_ok() {
        assert_eq!(compress("swiftlint", "").unwrap(), "swiftlint: ok");
    }
}
