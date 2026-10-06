//! Serde helpers and `#[serde(default = "...")]` fns for `Config`.

use serde::Deserialize;

/// `compression_level`: a number in `[0, 1]`, clamped.
pub(super) fn deserialize_compression_level<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(f64::deserialize(deserializer)?.clamp(0.0, 1.0))
}

pub(super) fn default_compression_level() -> f64 {
    super::DEFAULT_COMPRESSION_LEVEL
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if signature
pub(super) fn is_default_compression_level(v: &f64) -> bool {
    super::is_default_compression_level_value(*v)
}

pub(super) fn default_true() -> bool {
    true
}
