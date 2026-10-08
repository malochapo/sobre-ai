use crate::core::preservation;
use crate::core::tokens::count_tokens;

macro_rules! static_regex {
    ($pattern:expr_2021) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| {
            regex::Regex::new($pattern).expect(concat!("BUG: invalid static regex: ", $pattern))
        })
    }};
}

const MIN_DENSITY: f64 = 0.15;

#[derive(Debug, Clone)]
pub struct QualityScore {
    pub ast_score: f64,
    pub identifier_score: f64,
    pub line_score: f64,
    pub density: f64,
    pub composite: f64,
    pub passed: bool,
}

impl QualityScore {
    pub fn format_compact(&self) -> String {
        if self.passed {
            format!(
                "Q:{:.0}% (ast:{:.0} id:{:.0} ln:{:.0} ρ:{:.0}) ✓",
                self.composite * 100.0,
                self.ast_score * 100.0,
                self.identifier_score * 100.0,
                self.line_score * 100.0,
                self.density * 100.0,
            )
        } else {
            format!(
                "Q:{:.0}% (ast:{:.0} id:{:.0} ln:{:.0} ρ:{:.0}) ✗ BELOW THRESHOLD",
                self.composite * 100.0,
                self.ast_score * 100.0,
                self.identifier_score * 100.0,
                self.line_score * 100.0,
                self.density * 100.0,
            )
        }
    }
}

/// Information density: ratio of semantic tokens to total output tokens.
/// Measures how much "meaning" per output token is preserved.
pub fn information_density(original: &str, compressed: &str, ext: &str) -> f64 {
    let output_tokens = count_tokens(compressed);
    if output_tokens == 0 {
        return 1.0;
    }

    let pres = preservation::measure(original, compressed, ext);
    let semantic_items = pres.functions_preserved + pres.exports_preserved + pres.imports_preserved;
    let ident_re = static_regex!(r"\b[a-zA-Z_][a-zA-Z0-9_]{3,}\b");
    let unique_idents: std::collections::HashSet<&str> =
        ident_re.find_iter(compressed).map(|m| m.as_str()).collect();
    let semantic_token_estimate = semantic_items + unique_idents.len();

    (semantic_token_estimate as f64 / output_tokens as f64).min(1.0)
}

/// Density-specific guard that does NOT penalise line reduction.
///
/// The standard `score()` composite weights `line_score` at 20% and the
/// adaptive threshold rises as `line_score` drops — so a density target of
/// 0.4 (which *intentionally* discards ~60% of lines) always fails by
/// construction. Density mode needs a structure-only gate: AST + identifiers.
pub fn guard_density(
    original: &str,
    compressed: &str,
    ext: &str,
    target: f64,
) -> (String, QualityScore) {
    let pres = preservation::measure(original, compressed, ext);
    let ast_score = pres.overall();
    let identifier_score = measure_identifier_preservation(original, compressed);
    let line_score = measure_line_preservation(original, compressed);
    let density = information_density(original, compressed, ext);

    // Structure-only composite: ignore line_score for density mode.
    let structure_composite = ast_score * 0.6 + identifier_score * 0.4;

    // Fixed threshold: only reject when real structure is damaged, not when
    // we kept fewer lines (which is the *point* of density mode).
    let structure_threshold = 0.85;
    let passed = structure_composite >= structure_threshold && density >= MIN_DENSITY;

    let q = QualityScore {
        ast_score,
        identifier_score,
        line_score,
        density,
        composite: structure_composite,
        passed,
    };

    if passed {
        (compressed.to_string(), q)
    } else {
        tracing::debug!(
            target = target,
            composite = structure_composite,
            threshold = structure_threshold,
            "density quality gate rejected → full fallback"
        );
        (original.to_string(), q)
    }
}

fn measure_identifier_preservation(original: &str, compressed: &str) -> f64 {
    let ident_re = static_regex!(r"\b[a-zA-Z_][a-zA-Z0-9_]{3,}\b");

    let original_idents: std::collections::HashSet<&str> =
        ident_re.find_iter(original).map(|m| m.as_str()).collect();

    if original_idents.is_empty() {
        return 1.0;
    }

    let preserved = original_idents
        .iter()
        .filter(|id| compressed.contains(*id))
        .count();

    preserved as f64 / original_idents.len() as f64
}

fn measure_line_preservation(original: &str, compressed: &str) -> f64 {
    let original_lines: usize = original.lines().filter(|l| !l.trim().is_empty()).count();
    if original_lines == 0 {
        return 1.0;
    }

    let compressed_lines: usize = compressed.lines().filter(|l| !l.trim().is_empty()).count();
    let ratio = compressed_lines as f64 / original_lines as f64;

    ratio.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_density_high_for_meaningful_compression() {
        let original = "pub fn calculate_total(items: Vec<Item>) -> f64 {\n    items.iter().map(|i| i.price * i.quantity as f64).sum()\n}\n";
        let d = information_density(original, original, "rs");
        assert!(d > 0.15, "identity should have high density: {d}");
    }

    #[test]
    fn test_density_low_for_garbage() {
        let original = "pub fn calculate_total(items: Vec<Item>) -> f64 {\n    items.iter().map(|i| i.price).sum()\n}\n";
        let garbage = "xxx xxx xxx xxx xxx xxx xxx xxx xxx xxx xxx xxx xxx xxx xxx";
        let d = information_density(original, garbage, "rs");
        assert!(d < 0.5, "garbage output should have low density: {d}");
    }
}
