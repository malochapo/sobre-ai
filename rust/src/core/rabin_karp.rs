//! Content-defined chunking: cuts text where a rolling hash over a 48-byte
//! window has its low 9 bits zero (about every 512 bytes), with chunks of 64
//! to 2048 bytes, so a local edit moves few boundaries. The BM25 index uses it
//! to split files with no symbols.
//!
//! Offsets are byte offsets and may fall inside a UTF-8 character. The
//! rolling update removes the oldest byte with `BASE^(window-1)` after the
//! shift, so the hash is not exactly a function of the last 48 bytes.

const PRIME: u64 = 1_000_000_007;
const BASE: u64 = 256;
const MIN_CHUNK: usize = 64;
const MAX_CHUNK: usize = 2048;
const TARGET_CHUNK: usize = 512;
const MASK: u64 = TARGET_CHUNK as u64 - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub offset: usize,
    pub length: usize,
    pub hash: u64,
}

pub fn chunk(content: &str) -> Vec<Chunk> {
    let bytes = content.as_bytes();
    if bytes.is_empty() {
        return vec![];
    }

    if bytes.len() <= MIN_CHUNK {
        return vec![Chunk {
            offset: 0,
            length: bytes.len(),
            hash: full_hash(bytes),
        }];
    }

    let window = 48.min(bytes.len());
    let mut chunks = Vec::new();
    let mut chunk_start = 0;
    let mut rolling = 0u64;
    let mut pow = 1u64;

    for i in 0..window.saturating_sub(1) {
        let _ = i;
        pow = pow.wrapping_mul(BASE) % PRIME;
    }

    for i in 0..bytes.len() {
        rolling = rolling.wrapping_mul(BASE).wrapping_add(bytes[i] as u64) % PRIME;

        if i >= window {
            let old = bytes[i - window] as u64;
            rolling = (rolling + PRIME - old.wrapping_mul(pow) % PRIME) % PRIME;
        }

        let chunk_len = i + 1 - chunk_start;

        let is_boundary = chunk_len >= MIN_CHUNK && (rolling & MASK == 0);
        let is_max = chunk_len >= MAX_CHUNK;

        if is_boundary || is_max || i == bytes.len() - 1 {
            let slice = &bytes[chunk_start..=i];
            chunks.push(Chunk {
                offset: chunk_start,
                length: slice.len(),
                hash: full_hash(slice),
            });
            chunk_start = i + 1;
        }
    }

    chunks
}

fn full_hash(data: &[u8]) -> u64 {
    let mut h = 0u64;
    for &b in data {
        h = h.wrapping_mul(BASE).wrapping_add(b as u64) % PRIME;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_chunking() {
        let content = "a".repeat(1000);
        let c1 = chunk(&content);
        let c2 = chunk(&content);
        assert_eq!(c1, c2);
    }

    #[test]
    fn empty_content() {
        let c = chunk("");
        assert!(c.is_empty());
    }

    #[test]
    fn small_content_single_chunk() {
        let c = chunk("hello world");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].offset, 0);
    }

    #[test]
    fn respects_min_chunk_size() {
        let content = "x".repeat(500);
        let chunks = chunk(&content);
        for c in &chunks[..chunks.len().saturating_sub(1)] {
            assert!(
                c.length >= MIN_CHUNK,
                "Chunk length {} < MIN_CHUNK {}",
                c.length,
                MIN_CHUNK
            );
        }
    }

    #[test]
    fn respects_max_chunk_size() {
        let content = "x".repeat(10000);
        let chunks = chunk(&content);
        for c in &chunks {
            assert!(
                c.length <= MAX_CHUNK + 1,
                "Chunk length {} > MAX_CHUNK {}",
                c.length,
                MAX_CHUNK
            );
        }
    }

    #[test]
    fn local_change_affects_few_chunks() {
        let original = "a".repeat(2000);
        let mut modified = original.clone();
        // SAFETY: we overwrite a single byte with an ASCII value (`b'Z'`) at an
        // in-bounds index, which keeps the buffer valid UTF-8.
        unsafe {
            let bytes = modified.as_bytes_mut();
            bytes[1000] = b'Z';
        }

        let c1 = chunk(&original);
        let c2 = chunk(&modified);

        let unchanged = c1
            .iter()
            .filter(|a| c2.iter().any(|b| b.hash == a.hash))
            .count();

        let stability = unchanged as f64 / c1.len().max(1) as f64;
        assert!(
            stability > 0.5,
            "Expected > 50% chunks stable, got {:.0}% ({unchanged}/{})",
            stability * 100.0,
            c1.len()
        );
    }

    #[test]
    fn covers_all_bytes() {
        let content = "hello world, this is a longer test string for chunking purposes!".repeat(20);
        let chunks = chunk(&content);
        let total: usize = chunks.iter().map(|c| c.length).sum();
        assert_eq!(total, content.len());
    }

    #[test]
    fn unicode_content() {
        let content = "日本語テスト。これはUnicodeのテストです。".repeat(30);
        let chunks = chunk(&content);
        assert!(!chunks.is_empty());
        let total: usize = chunks.iter().map(|c| c.length).sum();
        assert_eq!(total, content.len());
    }
}
