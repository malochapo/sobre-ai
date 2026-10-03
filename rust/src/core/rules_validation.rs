//! Rules-file parsing, validation, and merge helpers.
use crate::core::config::CompressionLevel;

use super::rules_canonical::{END_MARK, RULES_VERSION, START_MARK, Wrapper, render};

/// A parsed lean-ctx rules section from a file on disk.
///
/// Handles version detection, content boundary discovery, and prefix/suffix
/// extraction. This is the only place that parses rule markers.
#[derive(Debug)]
pub struct RulesFile<'a> {
    content: &'a str,
    /// Byte offset of `START_MARK` (or the first old-format marker found).
    start: Option<usize>,
    /// Byte offset of `END_MARK`.
    end: Option<usize>,
    /// Parsed version number (0 if no version comment found).
    version: usize,
}

/// Parse the version number from the first version comment found.
fn parse_version_number(s: &str) -> Option<usize> {
    let prefix = "<!-- version: ";
    let vs = s.find(prefix)?;
    let num_start = vs + prefix.len();
    let end = s[num_start..].find(" -->")?;
    s[num_start..num_start + end].parse().ok()
}

impl<'a> RulesFile<'a> {
    /// Parse `content`, scanning for `START_MARK` and version comment.
    pub fn parse(content: &'a str) -> Self {
        let start = content.find(START_MARK);
        let version = start
            .and_then(|s| parse_version_number(&content[s + START_MARK.len()..]))
            .unwrap_or(0);
        let end = content.find(END_MARK);
        RulesFile {
            content,
            start,
            end,
            version,
        }
    }

    /// Whether the file carries any lean-ctx rules content.
    pub fn has_content(&self) -> bool {
        self.start.is_some()
    }

    /// The detected version (0 if no version marker).
    pub fn version(&self) -> usize {
        self.version
    }

    /// Whether the file's version is at least `RULES_VERSION`.
    pub fn is_current(&self) -> bool {
        self.version >= RULES_VERSION
    }

    /// Content before the first `START_MARK`.
    pub fn prefix(&self) -> &'a str {
        self.start.map_or("", |s| self.content[..s].trim())
    }

    /// Content after the last `END_MARK`.
    pub fn suffix(&self) -> &'a str {
        self.end
            .map_or("", |e| self.content[e + END_MARK.len()..].trim())
    }

    /// The lean-ctx block on disk, when both markers are present.
    fn block(&self) -> Option<&'a str> {
        match (self.start, self.end) {
            (Some(s), Some(e)) if e >= s => Some(&self.content[s..e + END_MARK.len()]),
            _ => None,
        }
    }

    /// Whether the on-disk block matches a fresh `render`.
    pub fn block_matches_render(
        &self,
        shadow: bool,
        wrapper: Wrapper,
        level: CompressionLevel,
    ) -> bool {
        match self.block() {
            Some(block) => block.trim() == render(shadow, wrapper, level).trim(),
            None => false,
        }
    }

    /// Merge freshly-rendered rules into this file.
    pub fn merged(&self, shadow: bool, wrapper: Wrapper, level: CompressionLevel) -> String {
        let fresh = render(shadow, wrapper, level);
        if self.start.is_some() {
            let before = self.prefix();
            let after = self.suffix();
            let mut out = String::new();
            if !before.is_empty() {
                out.push_str(before);
                out.push('\n');
                out.push('\n');
            }
            out.push_str(&fresh);
            if !after.is_empty() {
                out.push('\n');
                out.push('\n');
                out.push_str(after);
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out
        } else {
            let trimmed = self.content.trim_end();
            let mut out = trimmed.to_string();
            if !out.is_empty() {
                out.push('\n');
                out.push('\n');
            }
            out.push_str(&fresh);
            out
        }
    }

    /// Create initial rules content.
    pub fn initial(shadow: bool, wrapper: Wrapper, level: CompressionLevel) -> String {
        render(shadow, wrapper, level)
    }
}
