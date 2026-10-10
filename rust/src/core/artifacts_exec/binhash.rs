//! SHA-256 of a file, used to check the ONNX Runtime download against its pin
//! (`ort_provision`).
//!
//! SHA-256 (not the engine's internal BLAKE3) is deliberate: the pin is the
//! value an ordinary `sha256sum` prints, so it is reproducible without sobre.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of a byte buffer: the downloaded ONNX Runtime archive.
#[cfg(feature = "dense")]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    crate::core::hasher::hex_encode(&Sha256::digest(bytes))
}

/// Stream-hash a file and return its lowercase hex SHA-256. Streaming (8 KiB
/// chunks) keeps memory flat regardless of binary size.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read {} failed: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(crate::core::hasher::hex_encode(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A unique scratch dir per test (PID + label) so parallel tests never share
    /// — and clean up — the same directory.
    fn tmp(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sobre-binhash-{}-{label}", std::process::id()))
    }

    #[test]
    fn hashes_file_matching_known_sha256() {
        let dir = tmp("known");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("payload.bin");
        std::fs::write(&f, b"abc").unwrap();
        // Known SHA-256 of "abc".
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_file(&f).unwrap(), expected);
        std::fs::remove_dir_all(&dir).ok();
    }
}
