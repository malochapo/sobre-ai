//! Per-item sensitivity model.
//!
//! Assigns a [`SensitivityLevel`] to context items (knowledge facts, file
//! paths) from path + content signals, stored with each fact. Only
//! high-precision signals raise a level: detected secrets → `Secret`; Luhn-validated card numbers and IBANs → `Confidential`. No
//! speculative heuristics.

mod classify;

pub use classify::classify_content;

use serde::{Deserialize, Serialize};

/// Ordered sensitivity classification.
///
/// The derived `Ord` drives every `level >= floor` comparison, so the
/// declaration order is significant: `Public < Internal < Confidential < Secret`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum SensitivityLevel {
    /// Safe to send to the model (default).
    #[default]
    Public,
    /// Internal-only material; reserved for explicit/manual tagging.
    Internal,
    /// Personally identifiable / regulated data (card numbers, IBANs).
    Confidential,
    /// Secrets and credentials. Must never reach the model when enforced.
    Secret,
}

impl SensitivityLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            SensitivityLevel::Public => "public",
            SensitivityLevel::Internal => "internal",
            SensitivityLevel::Confidential => "confidential",
            SensitivityLevel::Secret => "secret",
        }
    }

    /// Tolerant parse from a config/env string. Returns `None` on unknown input
    /// so callers can fall back to the default without panicking.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "public" | "none" | "" => Some(SensitivityLevel::Public),
            "internal" => Some(SensitivityLevel::Internal),
            "confidential" | "pii" => Some(SensitivityLevel::Confidential),
            "secret" | "secrets" | "credential" | "credentials" => Some(SensitivityLevel::Secret),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered() {
        assert!(SensitivityLevel::Public < SensitivityLevel::Internal);
        assert!(SensitivityLevel::Internal < SensitivityLevel::Confidential);
        assert!(SensitivityLevel::Confidential < SensitivityLevel::Secret);
    }

    #[test]
    fn parse_is_tolerant() {
        assert_eq!(
            SensitivityLevel::parse("SECRET"),
            Some(SensitivityLevel::Secret)
        );
        assert_eq!(
            SensitivityLevel::parse("pii"),
            Some(SensitivityLevel::Confidential)
        );
        assert_eq!(SensitivityLevel::parse(""), Some(SensitivityLevel::Public));
        assert_eq!(SensitivityLevel::parse("nope"), None);
    }
}
