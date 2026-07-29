//! Token-set similarity — one implementation, one place.
//!
//! sobre found this algorithm written twice: `memory_consolidation::token_jaccard`
//! (f64) and `memory_lifecycle::word_similarity` (f32). Same lowercase +
//! whitespace-split + Jaccard, differing only in float width and in what they
//! returned for two empty strings. Two similarity functions over the same
//! knowledge facts is exactly how two memory subsystems drift apart, so there is
//! now one.

use std::collections::HashSet;

/// Jaccard similarity over whitespace-split, case-folded tokens.
///
/// Returns `1.0` when both sides are empty (they are trivially identical) and
/// `0.0` when exactly one is. That empty-pair convention comes from the
/// `memory_lifecycle` version; the `memory_consolidation` version returned
/// `0.0` for two empty strings, which reported identical inputs as unrelated.
#[must_use]
pub fn token_jaccard(a: &str, b: &str) -> f64 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_tokens: HashSet<&str> = a_lower.split_whitespace().collect();
    let b_tokens: HashSet<&str> = b_lower.split_whitespace().collect();

    if a_tokens.is_empty() && b_tokens.is_empty() {
        return 1.0;
    }

    let union = a_tokens.union(&b_tokens).count();
    if union == 0 {
        return 0.0;
    }
    let intersection = a_tokens.intersection(&b_tokens).count();

    #[allow(clippy::cast_precision_loss)]
    {
        intersection as f64 / union as f64
    }
}

/// [`token_jaccard`] narrowed to `f32`, for callers whose thresholds are `f32`.
#[must_use]
pub fn token_jaccard_f32(a: &str, b: &str) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        token_jaccard(a, b) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_strings_score_one() {
        assert!((token_jaccard("hello world", "hello world") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn partial_overlap_is_between() {
        let sim = token_jaccard("uses PostgreSQL database", "PostgreSQL database system");
        assert!(sim > 0.0 && sim < 1.0, "got {sim}");
    }

    #[test]
    fn disjoint_strings_score_zero() {
        assert!(token_jaccard("Redis cache", "Docker compose") < 0.01);
    }

    #[test]
    fn case_and_whitespace_are_folded() {
        assert!((token_jaccard("Hello   WORLD", "world hello") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn both_empty_are_identical_not_unrelated() {
        assert!((token_jaccard("", "   ") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn one_empty_side_is_unrelated() {
        assert!(token_jaccard("", "something") < 1e-9);
    }

    #[test]
    fn f32_narrowing_matches() {
        assert!((token_jaccard_f32("a b", "b c") - 1.0 / 3.0).abs() < 1e-6);
    }
}
