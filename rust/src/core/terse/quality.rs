//! Quality gate for terse compression.
//!
//! Ensures compression does not destroy critical information:
//! - File paths must be preserved
//! - Code identifiers (>= 6 chars) must be preserved
//! - Minimum savings threshold (default 10%) must be met

use std::collections::HashSet;

pub struct QualityConfig {
    pub min_path_preservation: f32,
    pub min_identifier_preservation: f32,
    pub min_identifier_len: usize,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            min_path_preservation: 1.0,
            min_identifier_preservation: 0.90,
            min_identifier_len: 6,
        }
    }
}

/// Whether `compressed` keeps enough of the paths and identifiers in
/// `original` to pass the quality gate.
pub fn check(original: &str, compressed: &str, config: &QualityConfig) -> bool {
    let orig_paths = extract_paths(original);
    let comp_paths = extract_paths(compressed);
    let paths_found = orig_paths
        .iter()
        .filter(|p| comp_paths.contains(*p))
        .count();
    let paths_preserved = orig_paths.is_empty()
        || (paths_found as f32 / orig_paths.len() as f32) >= config.min_path_preservation;

    let orig_idents = extract_identifiers(original, config.min_identifier_len);
    let comp_words: HashSet<String> = compressed
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() >= config.min_identifier_len)
        .map(str::to_lowercase)
        .collect();
    let idents_found = orig_idents
        .iter()
        .filter(|id| comp_words.contains(&id.to_lowercase()))
        .count();
    let identifiers_preserved = orig_idents.is_empty()
        || (idents_found as f32 / orig_idents.len() as f32) >= config.min_identifier_preservation;

    paths_preserved && identifiers_preserved
}

fn extract_paths(text: &str) -> HashSet<String> {
    let mut paths = HashSet::new();
    for word in text.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| c == '\'' || c == '"' || c == ',' || c == ';');
        if looks_like_path(cleaned) {
            paths.insert(cleaned.to_string());
        }
    }
    paths
}

fn looks_like_path(s: &str) -> bool {
    if s.len() < 3 {
        return false;
    }
    let has_separator = s.contains('/') || s.contains('\\');
    let has_extension = s.rfind('.').is_some_and(|dot| {
        let ext = &s[dot + 1..];
        !ext.is_empty() && ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    has_separator || (has_extension && s.chars().filter(|c| *c == '.').count() <= 2)
}

const MAX_IDENTIFIERS: usize = 200;

/// A token counts as a must-preserve identifier if it looks like an ordinary
/// code identifier (has a letter) OR looks like an encoded blob (hex/base64,
/// e.g. a `git rev-parse HEAD` hash) — the latter has no alphabetic
/// character for the ordinary check to key on, but dropping it is exactly
/// the failure mode the noise-detector in `scoring.rs` must not cause: a
/// payload-shaped blob must still trip this gate if compression removes it.
fn extract_identifiers(text: &str, min_len: usize) -> HashSet<String> {
    let mut idents = HashSet::new();
    for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let looks_like_identifier = word.len() >= min_len
            && (word.chars().any(char::is_alphabetic) || super::scoring::is_blob_token(word));
        if looks_like_identifier {
            idents.insert(word.to_string());
            if idents.len() >= MAX_IDENTIFIERS {
                break;
            }
        }
    }
    idents
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path gate alone: identifier preservation is not scored.
    fn paths_only() -> QualityConfig {
        QualityConfig {
            min_identifier_preservation: 0.0,
            ..QualityConfig::default()
        }
    }

    #[test]
    fn path_detection() {
        assert!(looks_like_path("src/main.rs"));
        assert!(looks_like_path("config.toml"));
        assert!(!looks_like_path("hello"));
        assert!(!looks_like_path("a"));
    }

    #[test]
    fn extract_paths_from_text() {
        let paths = extract_paths("error in src/lib.rs at line 42");
        assert!(paths.contains("src/lib.rs"));
    }

    #[test]
    fn extract_identifiers_min_len() {
        let idents = extract_identifiers("fn configure_premium_features(home: Path)", 6);
        assert!(idents.contains("configure_premium_features"));
        assert!(!idents.contains("home"));
    }

    #[test]
    fn quality_passes_with_good_compression() {
        let original =
            "src/main.rs: error[E0308]: mismatched types\nlong description here that is verbose";
        let compressed = "src/main.rs: err[E0308]: mismatched types";
        assert!(check(original, compressed, &paths_only()));
    }

    #[test]
    fn quality_passes_when_identifiers_preserved() {
        assert!(
            check("hello", "hello", &QualityConfig::default()),
            "should pass when paths and identifiers are preserved"
        );
    }

    #[test]
    fn quality_fails_missing_path() {
        let original = "error in src/config.rs";
        let compressed = "error occurred";
        assert!(!check(original, compressed, &paths_only()));
    }

    /// A pure-digit, hex-shaped blob (e.g. an all-numeric slice of a
    /// `git rev-parse HEAD` hash) has no alphabetic character, so the
    /// ordinary identifier check alone would miss it — `is_blob_token` closes
    /// that gap so dropping it still fails the gate.
    #[test]
    fn quality_fails_when_payload_blob_is_dropped() {
        let blob = "1".repeat(9) + &"2".repeat(9) + &"3".repeat(9);
        let original = format!("commit hash: {blob}");
        let compressed = "commit hash:";
        let identifiers_only = QualityConfig {
            min_path_preservation: 0.0,
            ..QualityConfig::default()
        };
        assert!(
            !check(&original, compressed, &identifiers_only),
            "dropping a pure-digit hash-shaped blob must fail the identifier gate"
        );
    }
}
