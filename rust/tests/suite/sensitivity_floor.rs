//! Integration tests for the per-item sensitivity classification (#212).
//!
//! Exercises the public `core::sensitivity` API on real secret/PII vectors.
//! All deterministic — no env, no config files, no global state.

use lean_ctx::core::sensitivity::{SensitivityLevel, classify, classify_content};
use std::path::Path;

#[test]
fn classification_vectors_are_precise() {
    assert_eq!(
        classify_content("nothing sensitive here, just text"),
        SensitivityLevel::Public
    );
    // Non-Luhn 16-digit number must NOT be flagged.
    assert_eq!(
        classify_content("invoice 1234567890123456"),
        SensitivityLevel::Public
    );
    // Valid IBAN (mod-97) → Confidential.
    assert_eq!(
        classify_content("iban DE89370400440532013000"),
        SensitivityLevel::Confidential
    );
    // Secret beats path-public.
    assert_eq!(
        classify(
            Some(Path::new("src/main.rs")),
            "key = ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        ),
        SensitivityLevel::Secret
    );
}
