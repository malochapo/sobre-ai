//! Serde helpers and `#[serde(default = "...")]` fns for `Config`.

use serde::Deserialize;

/// `compression_level`: a number in `[0, 1]` (clamped), or a legacy level name
/// read as the value of its band.
pub(super) fn deserialize_compression_level<'de, D>(
    deserializer: D,
) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(n) => n
            .as_f64()
            .map(|v| Some(v.clamp(0.0, 1.0)))
            .ok_or_else(|| D::Error::custom("compression_level must be a number in [0, 1]")),
        serde_json::Value::String(s) => super::CompressionLevel::from_str_label(&s)
            .map(|level| Some(level.value()))
            .ok_or_else(|| D::Error::custom(format!("unknown compression_level: {s}"))),
        serde_json::Value::Null => Ok(None),
        _ => Err(D::Error::custom(
            "compression_level must be a number in [0, 1]",
        )),
    }
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
