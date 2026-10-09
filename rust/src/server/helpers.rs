use serde_json::Value;

pub fn get_str(args: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    args?
        .get(key)?
        .as_str()
        .map(std::string::ToString::to_string)
}

pub fn get_bool(args: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<bool> {
    args?.get(key)?.as_bool()
}

/// A content hash that reads at most the first and last 4 KiB of a long
/// string. Nothing in sobre calls it: only the integration tests exercise it.
#[cfg(feature = "test-support")]
pub fn hash_fast(s: &str) -> String {
    const THRESHOLD: usize = 16 * 1024;
    if s.len() <= THRESHOLD {
        crate::core::hasher::hash_str(s)
    } else {
        let prefix = &s[..s.floor_char_boundary(4096)];
        let suffix = &s[s.ceil_char_boundary(s.len().saturating_sub(4096))..];
        let mut hasher = blake3::Hasher::new();
        hasher.update(prefix.as_bytes());
        hasher.update(s.len().to_string().as_bytes());
        hasher.update(suffix.as_bytes());
        hasher.finalize().to_hex().to_string()
    }
}
