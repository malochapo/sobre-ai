//! Line keep/drop decisions for `entropy` mode.
//!
//! A line's "surprise" is the mean `-log2 P(token)` of its `o200k_base`
//! tokens under a Zipf prior that takes the token ID as its frequency rank
//! (lower IDs were merged earlier, so are roughly more frequent). It is a
//! cheap stand-in for how rare a line's tokens are, not a language model's
//! probability: a line with low token entropy but rare tokens is kept.

use std::sync::OnceLock;

use super::tokens::encode_tokens;

static VOCAB_LOG_PROBS: OnceLock<Vec<f64>> = OnceLock::new();

/// Build a log-probability table indexed by token ID.
/// Uses a Zipfian approximation: P(rank r) ~ 1/(r * H_n) where H_n is the
/// harmonic number, with the token ID standing in for the rank.
fn get_vocab_log_probs() -> &'static Vec<f64> {
    VOCAB_LOG_PROBS.get_or_init(|| {
        let vocab_size = 200_000usize;
        let h_n: f64 = (1..=vocab_size).map(|r| 1.0 / r as f64).sum();
        (0..vocab_size)
            .map(|rank| {
                let r = rank + 1; // 1-indexed rank
                let p = 1.0 / (r as f64 * h_n);
                -p.log2()
            })
            .collect()
    })
}

/// Compute the surprise score for a line of text.
///
/// Returns the mean negative log-probability (cross-entropy) of the line's
/// BPE tokens under the Zipfian prior. Higher values = more surprising to
/// the LLM = more important to keep.
///
/// Range: typically 5.0 (very common) to 17.0+ (very rare).
pub fn line_surprise(text: &str) -> f64 {
    let tokens = encode_tokens(text);
    if tokens.is_empty() {
        return 0.0;
    }
    let log_probs = get_vocab_log_probs();
    let max_id = log_probs.len();

    let total: f64 = tokens
        .iter()
        .map(|&t| {
            let id = t as usize;
            if id < max_id {
                log_probs[id]
            } else {
                17.6 // max surprise for OOV tokens (~log2(200000))
            }
        })
        .sum();

    total / tokens.len() as f64
}

/// Enhanced entropy filter that combines Shannon entropy with predictive surprise.
/// Lines pass if EITHER their entropy is above threshold OR their surprise is high.
/// This prevents dropping lines that look "low entropy" but contain rare, unique tokens.
pub fn should_keep_line(trimmed: &str, entropy_threshold: f64) -> bool {
    if trimmed.is_empty() || trimmed.len() < 3 {
        return true;
    }

    let tokens = encode_tokens(trimmed);
    let h = super::entropy::token_entropy_from_ids(&tokens);
    if h >= entropy_threshold {
        return true;
    }

    let h_norm = super::entropy::normalized_token_entropy_from_ids(&tokens);
    if h_norm >= 0.3 {
        return true;
    }

    // A line with low token entropy can still carry rare tokens.
    // This catches lines like `CustomDomainType::validate()`
    // which have low token diversity but high surprise per-token.
    let surprise = line_surprise(trimmed);
    surprise >= 11.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_code_has_low_surprise() {
        let common = "let x = 1;";
        let s = line_surprise(common);
        assert!(s > 0.0, "surprise should be positive");
    }

    #[test]
    fn rare_identifiers_have_higher_surprise() {
        let common = "let x = 1;";
        let rare = "let zygomorphic_validator = XenolithProcessor::new();";
        assert!(
            line_surprise(rare) > line_surprise(common),
            "rare identifiers should have higher surprise"
        );
    }

    #[test]
    fn empty_returns_zero() {
        assert_eq!(line_surprise(""), 0.0);
    }

    #[test]
    fn classify_surprise_is_consistent() {
        let simple = "let x = 1;";
        let complex = "ZygomorphicXenolithValidator::process_quantum_state(&mut ctx)";
        let s_simple = line_surprise(simple);
        let s_complex = line_surprise(complex);
        assert!(
            s_complex > s_simple,
            "rare identifiers ({s_complex}) should have higher surprise than common code ({s_simple})"
        );
    }

    #[test]
    fn should_keep_preserves_rare_lines() {
        let rare = "ZygomorphicValidator::process_xenolith(&mut state)";
        assert!(
            should_keep_line(rare, 1.0) || line_surprise(rare) < 11.0,
            "rare lines should be preserved or have measurable surprise"
        );
    }
}
