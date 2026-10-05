//! Serde helpers and `#[serde(default = "...")]` fns for `Config`.

use serde::Deserialize;

/// `compression_level`: a number in `[0, 1]`, clamped.
pub(super) fn deserialize_compression_level<'de, D>(
    deserializer: D,
) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<f64>::deserialize(deserializer)?.map(|v| v.clamp(0.0, 1.0)))
}

pub(super) fn default_true() -> bool {
    true
}

pub(super) fn default_bm25_max_cache_mb() -> u64 {
    128
}

pub(super) fn default_cognition_loop_interval() -> u64 {
    3600
}

pub(super) fn default_cognition_loop_max_steps() -> u8 {
    9
}

pub(super) fn default_cognition_synthesis_min_cluster() -> usize {
    3
}
