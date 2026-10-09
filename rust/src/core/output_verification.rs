use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WarningKind {
    MissingPath,
    MangledIdentifier,
    TruncatedBlock,
}

impl std::fmt::Display for WarningKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPath => write!(f, "missing_path"),
            Self::MangledIdentifier => write!(f, "mangled_identifier"),
            Self::TruncatedBlock => write!(f, "truncated_block"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationWarning {
    pub kind: WarningKind,
    pub detail: String,
    pub severity: WarningSeverity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WarningSeverity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub pass: bool,
    pub warnings: Vec<VerificationWarning>,
    pub info_loss_score: f64,
    pub paths_checked: usize,
    pub identifiers_checked: usize,
}

impl VerificationResult {
    pub fn ok() -> Self {
        Self {
            pass: true,
            warnings: Vec::new(),
            info_loss_score: 0.0,
            paths_checked: 0,
            identifiers_checked: 0,
        }
    }
}

/// Checks that compression kept file paths, identifiers and block structure.
/// The result fails only on a High-severity warning.
pub fn verify_output(source: &str, compressed: &str) -> VerificationResult {
    if source.is_empty() || compressed.is_empty() {
        return VerificationResult::ok();
    }

    // No-op compression should never produce warnings.
    if source == compressed {
        return VerificationResult::ok();
    }

    let (mut warnings, paths_checked) = check_paths(source, compressed);
    let (id_warnings, identifiers_checked) = check_identifiers(source, compressed);
    warnings.extend(id_warnings);
    warnings.extend(check_structure(source, compressed));

    let total_checks = (paths_checked + identifiers_checked).max(1);
    let loss_items = warnings
        .iter()
        .filter(|w| w.severity == WarningSeverity::High)
        .count() as f64
        * 2.0
        + warnings
            .iter()
            .filter(|w| w.severity == WarningSeverity::Medium)
            .count() as f64;
    let info_loss_score = (loss_items / total_checks as f64).min(1.0);

    let pass = !warnings.iter().any(|w| w.severity == WarningSeverity::High);

    VerificationResult {
        pass,
        warnings,
        info_loss_score,
        paths_checked,
        identifiers_checked,
    }
}

fn check_paths(source: &str, compressed: &str) -> (Vec<VerificationWarning>, usize) {
    let paths = extract_file_paths(source);
    let mut warnings = Vec::new();

    for path in &paths {
        let basename = path.rsplit('/').next().unwrap_or(path);
        if !compressed.contains(basename) {
            warnings.push(VerificationWarning {
                kind: WarningKind::MissingPath,
                detail: format!("Path reference lost: {path}"),
                severity: WarningSeverity::Medium,
            });
        }
    }

    (warnings, paths.len())
}

fn check_identifiers(source: &str, compressed: &str) -> (Vec<VerificationWarning>, usize) {
    let identifiers = extract_identifiers(source);
    let mut warnings = Vec::new();
    let significant: Vec<&str> = identifiers
        .iter()
        .filter(|id| id.len() >= 4)
        .map(String::as_str)
        .collect();

    for id in &significant {
        if !compressed.contains(id) {
            warnings.push(VerificationWarning {
                kind: WarningKind::MangledIdentifier,
                detail: format!("Identifier lost: {id}"),
                severity: if id.len() >= 8 {
                    WarningSeverity::High
                } else {
                    WarningSeverity::Low
                },
            });
        }
    }

    (warnings, significant.len())
}

fn check_structure(source: &str, compressed: &str) -> Vec<VerificationWarning> {
    let mut warnings = Vec::new();

    let src_opens: usize = source.chars().filter(|&c| c == '{').count();
    let src_closes: usize = source.chars().filter(|&c| c == '}').count();
    let src_diff = (src_opens as i64 - src_closes as i64).unsigned_abs();

    let opens: usize = compressed.chars().filter(|&c| c == '{').count();
    let closes: usize = compressed.chars().filter(|&c| c == '}').count();
    if opens > 0 || closes > 0 {
        let diff = (opens as i64 - closes as i64).unsigned_abs();
        // Only warn if compression materially worsened structural balance.
        if diff > (src_diff + 2) && diff > 2 {
            warnings.push(VerificationWarning {
                kind: WarningKind::TruncatedBlock,
                detail: format!("Brace mismatch: {{ {opens} vs }} {closes}"),
                severity: WarningSeverity::Medium,
            });
        }
    }

    let src_parens_open: usize = source.chars().filter(|&c| c == '(').count();
    let src_parens_close: usize = source.chars().filter(|&c| c == ')').count();
    let src_parens_diff = (src_parens_open as i64 - src_parens_close as i64).unsigned_abs();

    let parens_open: usize = compressed.chars().filter(|&c| c == '(').count();
    let parens_close: usize = compressed.chars().filter(|&c| c == ')').count();
    if parens_open > 0 || parens_close > 0 {
        let diff = (parens_open as i64 - parens_close as i64).unsigned_abs();
        if diff > (src_parens_diff + 3) && diff > 3 {
            warnings.push(VerificationWarning {
                kind: WarningKind::TruncatedBlock,
                detail: format!("Paren mismatch: ( {parens_open} vs ) {parens_close}"),
                severity: WarningSeverity::Low,
            });
        }
    }

    warnings
}

fn extract_file_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let re = Regex::new(
        r#"(?:^|[\s"'`(,])([a-zA-Z0-9_./-]{2,}\.(?:rs|ts|tsx|js|jsx|py|go|java|rb|cpp|c|h|toml|yaml|yml|json|md))\b"#
    )
    .ok()
    .or_else(|| Regex::new(r"(\S+\.\w+)").ok());

    if let Some(re) = re {
        for cap in re.captures_iter(text) {
            if let Some(m) = cap.get(1) {
                let p = m.as_str().to_string();
                if !paths.contains(&p) && p.len() < 200 {
                    paths.push(p);
                }
            }
        }
    }
    paths
}

fn extract_identifiers(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let re = Regex::new(
        r"\b(fn|struct|enum|trait|type|class|function|const|let|var|def|pub)\s+([a-zA-Z_][a-zA-Z0-9_]*)"
    )
    .ok()
    .or_else(|| Regex::new(r"([a-zA-Z_]\w+)").ok());

    if let Some(re) = re {
        for cap in re.captures_iter(text) {
            if let Some(m) = cap.get(2) {
                let id = m.as_str().to_string();
                if !ids.contains(&id) {
                    ids.push(id);
                }
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
        assert!(r.pass);
    }

    #[test]
    fn identical_passes() {
        let src = "fn hello() { println!(\"world\"); }";
        let r = verify_output(src, src);
        assert!(r.pass);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn detects_missing_path() {
        let src = "import { foo } from src/utils/helper.ts";
        let compressed = "import foo";
        let r = verify_output(src, compressed);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.kind == WarningKind::MissingPath)
        );
    }

    #[test]
    fn detects_lost_identifier() {
        let src = "fn calculate_monthly_revenue(data: &[f64]) -> f64 { data.iter().sum() }";
        let compressed = "fn calc() -> f64 { sum }";
        let r = verify_output(src, compressed);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.kind == WarningKind::MangledIdentifier)
        );
    }

    #[test]
    fn detects_brace_mismatch() {
        let src = "fn a() { if true { b(); } } fn c() { d(); } fn e() { f(); }";
        let compressed = "fn a() { if true { b(); fn c() { d(); fn e() { f();";
        let r = verify_output(src, compressed);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.kind == WarningKind::TruncatedBlock)
        );
    }

    #[test]
    fn preserved_identifiers_pass() {
        let src = "fn process_data(input: Vec<u8>) -> Result<()> { Ok(()) }";
        let compressed = "fn process_data(input: Vec<u8>) -> Result<()>";
        let r = verify_output(src, compressed);
        let mangled = r
            .warnings
            .iter()
            .filter(|w| w.kind == WarningKind::MangledIdentifier)
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
    fn info_loss_score_bounded() {
        let src = "fn very_long_function_name_here() {}\nfn another_significant_fn() {}";
        let compressed = "compressed";
        let r = verify_output(src, compressed);
        assert!(r.info_loss_score >= 0.0);
        assert!(r.info_loss_score <= 1.0);
    }
}
