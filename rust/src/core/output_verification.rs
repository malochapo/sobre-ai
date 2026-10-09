//! Checks that condensed output kept what the agent may need from the
//! original: file paths, defined identifiers and block structure. Callers send
//! the original instead whenever a check finds a loss.

use std::sync::OnceLock;

use regex::Regex;

/// One thing the condensed text lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    MissingPath,
    MangledIdentifier,
    TruncatedBlock,
}

#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Empty when nothing was lost.
    pub warnings: Vec<WarningKind>,
}

/// Checks that `compressed` kept the file paths, the defined identifiers of
/// four characters or more, and the brace/paren balance of `source`.
pub fn verify_output(source: &str, compressed: &str) -> VerificationResult {
    // Nothing to compare, or a no-op compression.
    if source.is_empty() || compressed.is_empty() || source == compressed {
        return VerificationResult {
            warnings: Vec::new(),
        };
    }

    let mut warnings = check_paths(source, compressed);
    warnings.extend(check_identifiers(source, compressed));
    warnings.extend(check_structure(source, compressed));
    VerificationResult { warnings }
}

fn check_paths(source: &str, compressed: &str) -> Vec<WarningKind> {
    extract_file_paths(source)
        .into_iter()
        .filter(|path| {
            let basename = path.rsplit('/').next().unwrap_or(path);
            !compressed.contains(basename)
        })
        .map(|_| WarningKind::MissingPath)
        .collect()
}

fn check_identifiers(source: &str, compressed: &str) -> Vec<WarningKind> {
    extract_identifiers(source)
        .into_iter()
        .filter(|id| id.len() >= 4 && !compressed.contains(id.as_str()))
        .map(|_| WarningKind::MangledIdentifier)
        .collect()
}

/// Whether `compressed` unbalances `open`/`close` by more than `slack` beyond
/// what `source` already had.
fn worsens_balance(source: &str, compressed: &str, open: char, close: char, slack: u64) -> bool {
    let imbalance = |s: &str| {
        let opens = s.chars().filter(|&c| c == open).count() as i64;
        let closes = s.chars().filter(|&c| c == close).count() as i64;
        (opens - closes).unsigned_abs()
    };
    let diff = imbalance(compressed);
    diff > imbalance(source) + slack && diff > slack
}

fn check_structure(source: &str, compressed: &str) -> Vec<WarningKind> {
    let mut warnings = Vec::new();
    if worsens_balance(source, compressed, '{', '}', 2) {
        warnings.push(WarningKind::TruncatedBlock);
    }
    if worsens_balance(source, compressed, '(', ')', 3) {
        warnings.push(WarningKind::TruncatedBlock);
    }
    warnings
}

fn extract_file_paths(text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r#"(?:^|[\s"'`(,])([a-zA-Z0-9_./-]{2,}\.(?:rs|ts|tsx|js|jsx|py|go|java|rb|cpp|c|h|toml|yaml|yml|json|md))\b"#,
        )
        .expect("valid path regex")
    });
    let mut paths = Vec::new();
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let p = m.as_str().to_string();
            if !paths.contains(&p) && p.len() < 200 {
                paths.push(p);
            }
        }
    }
    paths
}

fn extract_identifiers(text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"\b(fn|struct|enum|trait|type|class|function|const|let|var|def|pub)\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        )
        .expect("valid identifier regex")
    });
    let mut ids = Vec::new();
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(2) {
            let id = m.as_str().to_string();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_passes() {
        let r = verify_output("", "");
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn identical_passes() {
        let src = "fn hello() { println!(\"world\"); }";
        let r = verify_output(src, src);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn detects_missing_path() {
        let src = "import { foo } from src/utils/helper.ts";
        let compressed = "import foo";
        let r = verify_output(src, compressed);
        assert!(r.warnings.contains(&WarningKind::MissingPath));
    }

    #[test]
    fn detects_lost_identifier() {
        let src = "fn calculate_monthly_revenue(data: &[f64]) -> f64 { data.iter().sum() }";
        let compressed = "fn calc() -> f64 { sum }";
        let r = verify_output(src, compressed);
        assert!(r.warnings.contains(&WarningKind::MangledIdentifier));
    }

    #[test]
    fn detects_brace_mismatch() {
        let src = "fn a() { if true { b(); } } fn c() { d(); } fn e() { f(); }";
        let compressed = "fn a() { if true { b(); fn c() { d(); fn e() { f();";
        let r = verify_output(src, compressed);
        assert!(r.warnings.contains(&WarningKind::TruncatedBlock));
    }

    #[test]
    fn preserved_identifiers_pass() {
        let src = "fn process_data(input: Vec<u8>) -> Result<()> { Ok(()) }";
        let compressed = "fn process_data(input: Vec<u8>) -> Result<()>";
        let r = verify_output(src, compressed);
        let mangled = r
            .warnings
            .iter()
            .filter(|w| **w == WarningKind::MangledIdentifier)
            .count();
        assert_eq!(mangled, 0);
    }

    #[test]
    fn extract_paths_finds_common_extensions() {
        let text = "see src/core/auth.rs and lib/utils.py for details";
        let paths = extract_file_paths(text);
        assert!(paths.iter().any(|p| p.contains("auth.rs")));
        assert!(paths.iter().any(|p| p.contains("utils.py")));
    }

    #[test]
    fn extract_identifiers_finds_functions() {
        let text = "fn calculate_total(x: i32) -> i32 { x }\nstruct UserProfile { name: String }";
        let ids = extract_identifiers(text);
        assert!(ids.contains(&"calculate_total".to_string()));
        assert!(ids.contains(&"UserProfile".to_string()));
    }

    #[test]
    fn every_lost_identifier_is_reported() {
        let src = "fn very_long_function_name_here() {}\nfn another_significant_fn() {}";
        let compressed = "compressed";
        let r = verify_output(src, compressed);
        let lost = r
            .warnings
            .iter()
            .filter(|w| **w == WarningKind::MangledIdentifier);
        assert_eq!(lost.count(), 2);
    }
}
