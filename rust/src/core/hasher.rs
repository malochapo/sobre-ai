//! Central hashing utility using BLAKE3: fast, collision-resistant 256-bit
//! digests.

/// Lowercase hex of arbitrary bytes (a digest another hasher produced).
pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Return a hex-encoded BLAKE3 hash of the input bytes.
#[inline]
pub fn hash_hex(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// Convenience: hash a string slice and return hex digest.
#[inline]
pub fn hash_str(s: &str) -> String {
    hash_hex(s.as_bytes())
}

/// Short hash (first 16 hex chars = 64 bits) for cache keys and fingerprints.
#[inline]
pub fn hash_short(s: &str) -> String {
    let full = blake3::hash(s.as_bytes()).to_hex();
    full[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let a = hash_hex(b"hello");
        let b = hash_hex(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn short_hash_length() {
        let s = hash_short("test");
        assert_eq!(s.len(), 16);
    }

    #[test]
    fn different_inputs_differ() {
        assert_ne!(hash_str("foo"), hash_str("bar"));
    }
}
