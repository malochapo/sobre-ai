//! Content salience for memory: a cheap, deterministic keyword score over a
//! fact/finding's raw text.
//!
//! It answers "how much signal does this string carry?" without any model call,
//! and feeds two consumers:
//! - the cognition loop's auto-promotion gate (only promote findings above a
//!   floor), and
//! - the write-time admission floor that keeps low-signal facts out of a
//!   capped store.
//!
//! Pure and total so it never perturbs output determinism: the same text
//! always scores the same value, independent of process state.

/// Boost table: substrings that mark a finding as high-signal (errors, security,
/// failures). A base score of [`BASE`] is always granted so a plain, valid fact
/// is never zero — the floor (when enabled) is what decides admission.
const BASE: u32 = 20;
const BOOSTS: &[(&str, u32)] = &[
    ("error", 25),
    ("failed", 25),
    ("panic", 30),
    ("assert", 20),
    ("forbidden", 25),
    ("timeout", 20),
    ("deadlock", 25),
    ("security", 25),
    ("vuln", 25),
    ("e0", 15), // Rust error codes often start with E0xxx.
];

/// Score the salience of a piece of memory text. Always `>= BASE`.
#[must_use]
pub fn text_salience(text: &str) -> u32 {
    let s = text.to_lowercase();
    let mut score = BASE;
    for (pat, b) in BOOSTS {
        if s.contains(pat) {
            score = score.saturating_add(*b);
        }
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_gets_base_only() {
        assert_eq!(text_salience("the database is postgres"), BASE);
    }

    #[test]
    fn high_signal_terms_boost_above_base() {
        assert!(text_salience("auth failed with a security error") > BASE);
        // Each distinct boost term stacks.
        assert!(
            text_salience("panic deadlock timeout") > text_salience("timeout"),
            "multiple boosts must accumulate"
        );
    }

    #[test]
    fn is_deterministic_and_case_insensitive() {
        assert_eq!(
            text_salience("SECURITY VULN"),
            text_salience("security vuln")
        );
        assert_eq!(text_salience("x"), text_salience("x"));
    }
}
