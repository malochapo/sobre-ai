//! Binary-hash pinning for stdio addons (P3 — supply-chain hardening).
//!
//! A stdio addon spawns a local executable. Pinning that binary's SHA-256 in
//! the manifest (`[mcp] sha256 = "…"`) closes the gap between *what was audited*
//! and *what actually runs*: if the file on `PATH` is swapped after install, the
//! hash no longer matches and the gateway refuses to spawn it.
//!
//! SHA-256 (not the engine's internal BLAKE3) is deliberate — an author pins the
//! value an ordinary `sha256sum my-mcp` / `shasum -a 256 my-mcp` prints, so the
//! pin is reproducible without lean-ctx.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

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
        std::env::temp_dir().join(format!("leanctx-binhash-{}-{label}", std::process::id()))
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
