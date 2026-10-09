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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered() {
        assert!(SensitivityLevel::Public < SensitivityLevel::Internal);
        assert!(SensitivityLevel::Internal < SensitivityLevel::Confidential);
        assert!(SensitivityLevel::Confidential < SensitivityLevel::Secret);
    }
}
