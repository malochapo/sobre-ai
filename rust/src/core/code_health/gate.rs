//! Edit-time code-health gate — shared by `ctx_edit`, `ctx_patch`, and the
//! native-edit hook.
//!
//! Drift prevention is the lever lean-ctx has that a post-hoc scanner does not:
//! it sits *in the edit path*, so it can flag a complexity regression at the
//! moment it is introduced. The gate is advisory: it never blocks a write. Its
//! notice text is deterministic (#498-safe).

use super::{cognitive_delta, format_gate_notice, worst_regression};

/// The advisory `[CODE HEALTH]` notice for an edit from `old`→`new` source in a
/// file with `ext`, or `None` when no function regressed past the threshold.
pub fn evaluate(old: &str, new: &str, ext: &str) -> Option<String> {
    evaluate_with(old, new, ext, crate::core::code_health::COGNITIVE_THRESHOLD)
}

/// [`evaluate`] with an explicit `threshold` — the unit-tested core.
pub fn evaluate_with(old: &str, new: &str, ext: &str, threshold: u32) -> Option<String> {
    let deltas = cognitive_delta(old, new, ext);
    let worst = worst_regression(&deltas, threshold)?;
    Some(format_gate_notice(worst, threshold))
}

#[cfg(all(test, feature = "tree-sitter"))]
mod tests {
    use super::*;

    const FLAT: &str = "fn f(a: bool) { if a {} }";
    // 1+2+3+4+5+6 = 21 cognitive → over the default threshold of 15.
    const DEEP: &str = "fn f(a: bool) { if a { if a { if a { if a { if a { if a {} } } } } } }";

    #[test]
    fn regression_yields_a_notice() {
        let notice = evaluate_with(FLAT, DEEP, "rs", 15).expect("a regression must notice");
        assert!(notice.contains("[CODE HEALTH]"));
    }

    #[test]
    fn no_regression_is_silent() {
        assert!(evaluate_with(FLAT, FLAT, "rs", 15).is_none());
    }
}
