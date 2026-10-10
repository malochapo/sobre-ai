//! Tool description compression: shortens the descriptions `tools/list`
//! returns as `compression_level` rises. Two modes:
//! - Terse: the first three lines, without `Example`, `Note:` and `See also`
//!   lines.
//! - Lazy: the first line only, cut at 80 characters.

use crate::core::config::CompressionLevel;

/// Compression mode for tool descriptions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DescriptionMode {
    Full,
    Terse,
    Lazy,
}

impl DescriptionMode {
    pub fn from_compression_level(level: &CompressionLevel) -> Self {
        match level {
            CompressionLevel::Off | CompressionLevel::Lite => Self::Full,
            CompressionLevel::Standard => Self::Terse,
            CompressionLevel::Max | CompressionLevel::Raw => Self::Lazy,
        }
    }
}

/// Compresses a single tool description according to the mode.
pub fn compress_description(name: &str, description: &str, mode: DescriptionMode) -> String {
    match mode {
        DescriptionMode::Full => description.to_string(),
        DescriptionMode::Terse => terse_description(description),
        DescriptionMode::Lazy => lazy_description(name, description),
    }
}

fn terse_description(desc: &str) -> String {
    let mut lines: Vec<&str> = desc.lines().collect();

    lines.retain(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty()
            && !trimmed.starts_with("Example")
            && !trimmed.starts_with("Note:")
            && !trimmed.starts_with("See also")
    });

    if lines.len() > 3 {
        lines.truncate(3);
    }

    lines.join("\n")
}

fn lazy_description(name: &str, desc: &str) -> String {
    let mut lines = desc.lines();
    let first_line = lines.next().unwrap_or(name);
    // The tool surface is fixed and has no tool that returns full docs, so the
    // summary points nowhere: the first line, cut at 80 chars.
    if first_line.len() > 80 {
        format!("{}…", &first_line[..first_line.floor_char_boundary(77)])
    } else {
        first_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_mode_unchanged() {
        let desc = "Read a file from disk with caching.";
        assert_eq!(
            compress_description("sobre_read", desc, DescriptionMode::Full),
            desc
        );
    }

    #[test]
    fn terse_mode_keeps_three_lines_without_examples_or_notes() {
        let desc = "Read a file.\nExample: sobre_read(path)\nSecond line.\nNote: cached.\nThird line.\nFourth line.";
        let result = compress_description("sobre_read", desc, DescriptionMode::Terse);
        assert_eq!(result, "Read a file.\nSecond line.\nThird line.");
    }

    #[test]
    fn lazy_mode_short() {
        let desc = "Read a file from disk with intelligent caching and compression modes.\nSupports 10 different read modes for optimal token efficiency.";
        let result = compress_description("sobre_read", desc, DescriptionMode::Lazy);
        assert_eq!(
            result,
            "Read a file from disk with intelligent caching and compression modes."
        );
    }

    #[test]
    fn lazy_mode_keeps_a_single_line() {
        // A description that already fits on one line is kept verbatim.
        let desc = "Directory tree (replaces ls/find). Compact maps.";
        let result = compress_description("sobre_tree", desc, DescriptionMode::Lazy);
        assert_eq!(result, desc, "single-line lazy desc must be verbatim");
    }

    #[test]
    fn lazy_mode_truncates_a_long_first_line() {
        let desc = "This is a deliberately long single-line tool description that exceeds eighty characters to force truncation.";
        let result = compress_description("sobre_x", desc, DescriptionMode::Lazy);
        assert!(result.ends_with('…'), "{result}");
        assert!(result.chars().count() <= 78, "{result}");
    }

    #[test]
    fn mode_from_compression_level() {
        assert_eq!(
            DescriptionMode::from_compression_level(&CompressionLevel::Off),
            DescriptionMode::Full
        );
        assert_eq!(
            DescriptionMode::from_compression_level(&CompressionLevel::Standard),
            DescriptionMode::Terse
        );
        assert_eq!(
            DescriptionMode::from_compression_level(&CompressionLevel::Max),
            DescriptionMode::Lazy
        );
    }
}
