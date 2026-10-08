//! Token counting: one tokenizer (tiktoken `o200k_base`) for every count the
//! usage ledger records, so savings on every path are in the same unit.

use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

static BPE_O200K: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn get_bpe_o200k() -> Option<&'static CoreBPE> {
    BPE_O200K
        .get_or_init(|| {
            tiktoken_rs::o200k_base()
                .inspect_err(|e| tracing::error!("failed to load o200k_base tokenizer: {e}"))
                .ok()
        })
        .as_ref()
}

/// Bytes per token assumed when the tokenizer fails to load.
const CHARS_PER_TOKEN_ESTIMATE: f64 = 3.5;

const TOKEN_CACHE_MAX: u64 = 4096;

fn token_cache() -> &'static moka::sync::Cache<u64, usize> {
    static CACHE: OnceLock<moka::sync::Cache<u64, usize>> = OnceLock::new();
    CACHE.get_or_init(|| {
        moka::sync::Cache::builder()
            .max_capacity(TOKEN_CACHE_MAX)
            .build()
    })
}

fn hash_text(text: &str) -> u64 {
    let h = blake3::hash(text.as_bytes());
    let b = h.as_bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Counts BPE tokens with `o200k_base`.
pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let key = hash_text(text);
    let cache = token_cache();
    if let Some(cached) = cache.get(&key) {
        return cached;
    }

    let count = match get_bpe_o200k() {
        Some(bpe) => bpe.encode_with_special_tokens(text).len(),
        None => (text.len() as f64 / CHARS_PER_TOKEN_ESTIMATE).ceil() as usize,
    };
    cache.insert(key, count);
    count
}

/// Encodes text into BPE token IDs (o200k_base).
pub fn encode_tokens(text: &str) -> Vec<u32> {
    if text.is_empty() {
        return Vec::new();
    }
    match get_bpe_o200k() {
        Some(bpe) => bpe.encode_with_special_tokens(text),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_tokens_empty_is_zero() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn encode_tokens_empty_is_empty() {
        assert!(encode_tokens("").is_empty());
    }

    #[test]
    fn count_tokens_matches_encoded_length() {
        let text = "hello world, Grüezi 🌍";
        let counted = count_tokens(text);
        assert_eq!(counted, encode_tokens(text).len());
        assert_eq!(counted, count_tokens(text), "cached count is the same");
    }

    #[test]
    fn hash_text_is_stable_for_long_strings() {
        let long = "abc🙂".repeat(300);
        assert_eq!(hash_text(&long), hash_text(&long));
        assert!(count_tokens(&long) > 0);
    }

    #[test]
    fn count_tokens_reference_snapshot_o200k() {
        // Reference counts captured at tiktoken-rs 0.6 baseline (Plan B Task 0).
        // o200k_base encoding tables are a fixed spec; counts MUST stay identical
        // across the 0.6→0.12 crate bump. A mismatch = silently wrong accounting.
        let cases: [(&str, usize); 5] = [
            ("", 0),
            ("hello world", 2),
            ("fn main() { println!(\"hello\"); }", 9),
            ("Grüezi 🌍 — café déjà vu", 9),
            (
                "use std::collections::HashMap;\nfn main() {\n    let mut map = HashMap::new();\n}",
                23,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(
                count_tokens(text),
                expected,
                "token count drift for {text:?}"
            );
        }
    }
}
