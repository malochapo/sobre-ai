//! Per-item sensitivity classification.
//!
//! Assigns a [`SensitivityLevel`] to a knowledge fact from its content,
//! stored with the fact. Only high-precision signals raise a level: detected
//! secrets → `Secret`; Luhn-validated card numbers and IBANs →
//! `Confidential`. Nothing reads the stored level yet: it decides nothing.

mod classify;

pub use classify::classify_content;

use serde::{Deserialize, Serialize};

/// Sensitivity of a stored item, from the content signals it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SensitivityLevel {
    /// No signal found (default).
    #[default]
    Public,
    /// Personally identifiable / regulated data (card numbers, IBANs).
    Confidential,
    /// Secrets and credentials.
    Secret,
}
