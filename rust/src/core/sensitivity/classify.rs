//! Classification + redaction helpers for the sensitivity model.
//!
//! Only high-precision signals raise a level, to avoid false positives:
//! - secret-like paths ([`is_secret_like`]) and detected secrets → `Secret`
//! - Luhn-validated card numbers and mod-97-validated IBANs → `Confidential`

use std::sync::OnceLock;

use regex::Regex;

use super::SensitivityLevel;
use crate::core::secret_detection;

/// Classify free text by content.
pub fn classify_content(content: &str) -> SensitivityLevel {
    if !secret_detection::detect_secrets(content).is_empty() {
        return SensitivityLevel::Secret;
    }
    if has_credit_card(content) || has_iban(content) {
        return SensitivityLevel::Confidential;
    }
    SensitivityLevel::Public
}

// ---- Card numbers (Luhn) ---------------------------------------------------

fn card_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").expect("valid card regex"))
}

/// Luhn checksum over decimal digits. Length 13–19 (standard PAN range).
fn luhn_valid(digits: &[u8]) -> bool {
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        if d > 9 {
            return false;
        }
        let mut v = d as u32;
        if double {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
        double = !double;
    }
    sum.is_multiple_of(10)
}

fn digits_of(s: &str) -> Vec<u8> {
    s.chars()
        .filter(char::is_ascii_digit)
        .map(|c| c as u8 - b'0')
        .collect()
}

fn has_credit_card(content: &str) -> bool {
    card_re()
        .find_iter(content)
        .any(|m| luhn_valid(&digits_of(m.as_str())))
}

// ---- IBANs (ISO 7064 mod-97) ----------------------------------------------

fn iban_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b[A-Za-z]{2}\d{2}[A-Za-z0-9]{11,30}\b").expect("valid iban regex")
    })
}

/// Validate an IBAN candidate via the ISO 7064 mod-97 checksum.
fn iban_valid(candidate: &str) -> bool {
    let s: String = candidate.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() < 15 || s.len() > 34 {
        return false;
    }
    if !s.is_char_boundary(4) {
        return false;
    }
    // Move the first four characters to the end, then map letters A..Z -> 10..35.
    let rearranged = format!("{}{}", &s[4..], &s[..4]);
    let mut rem: u32 = 0;
    for c in rearranged.chars() {
        if let Some(d) = c.to_digit(10) {
            rem = (rem * 10 + d) % 97;
        } else if c.is_ascii_alphabetic() {
            let val = (c.to_ascii_uppercase() as u8 - b'A' + 10) as u32;
            rem = (rem * 100 + val) % 97;
        } else {
            return false;
        }
    }
    rem == 1
}

fn has_iban(content: &str) -> bool {
    iban_re().find_iter(content).any(|m| iban_valid(m.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_content_is_secret() {
        // GitHub token shape is detected by secret_detection.
        let t = "export TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        assert_eq!(classify_content(t), SensitivityLevel::Secret);
    }

    #[test]
    fn benign_content_is_public() {
        assert_eq!(
            classify_content("the quick brown fox jumps over 12 lazy dogs"),
            SensitivityLevel::Public
        );
    }

    #[test]
    fn luhn_valid_card_is_confidential() {
        // 4111 1111 1111 1111 is the canonical Visa test number (Luhn-valid).
        assert_eq!(
            classify_content("card: 4111 1111 1111 1111 on file"),
            SensitivityLevel::Confidential
        );
    }

    #[test]
    fn random_16_digits_not_flagged_unless_luhn() {
        // 1234567890123456 is NOT Luhn-valid → must stay public.
        assert_eq!(
            classify_content("order id 1234567890123456"),
            SensitivityLevel::Public
        );
    }

    #[test]
    fn valid_iban_is_confidential() {
        // DE89 3704 0044 0532 0130 00 is a well-known valid test IBAN.
        assert_eq!(
            classify_content("pay to DE89370400440532013000 now"),
            SensitivityLevel::Confidential
        );
    }

    #[test]
    fn invalid_iban_not_flagged() {
        assert_eq!(
            classify_content("ref DE00370400440532013000"),
            SensitivityLevel::Public
        );
    }
}
