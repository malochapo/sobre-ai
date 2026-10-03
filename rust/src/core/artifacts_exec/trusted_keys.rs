//! Signing keys this machine trusts for addons.
//!
//! An addon carrying an Ed25519 signature is only loaded when its signing
//! key is one pinned here out-of-band — the SSH `known_hosts` model.
//! `grammar_registry` checks this before loading, and it is what makes
//! `addons.require_signature = true` satisfiable.
//!
//! Two sources, checked in order:
//! 1. `SOBRE_TRUSTED_KEYS` — comma-separated hex public keys;
//! 2. the pinned set in `<config_dir>/trusted-keys.toml`, edited by hand.
//!
//! Trust is a separate question from signature validity: verification proves
//! the bytes were signed by the embedded key; [`is_trusted`] proves that key
//! is one we accept. Both must hold.
//!
//! Came from the org-policy distribution plane, which was removed: pinning a
//! key is a local decision, central policy distribution is not.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Env override carrying one or more trusted public keys (hex,
/// comma-separated). Intended for MDM / fleet provisioning.
const TRUST_ENV: &str = "SOBRE_TRUSTED_KEYS";

/// One pinned signing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedKey {
    /// Who this key signs for (informational).
    pub publisher: String,
    /// Ed25519 public key, hex (64 chars).
    pub public_key: String,
    /// When it was pinned (RFC 3339) — for the audit conversation, not enforcement.
    pub added_at: String,
}

/// The pinned trust set, persisted as `trusted-keys.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default, rename = "trusted_key", skip_serializing_if = "Vec::is_empty")]
    pub trusted_keys: Vec<TrustedKey>,
}

/// Location of the pinned trust file (`<config_dir>/trusted-keys.toml`).
pub fn trust_path() -> Result<PathBuf, String> {
    Ok(crate::core::paths::config_dir()?.join("trusted-keys.toml"))
}

/// Load the pinned set. A missing file is the common (un-pinned) case and
/// yields an empty store, never an error.
pub fn load() -> Result<TrustStore, String> {
    let path = trust_path()?;
    if !path.exists() {
        return Ok(TrustStore::default());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Whether `public_key` (hex) is pinned — via the env override or the file.
#[must_use]
pub fn is_trusted(public_key: &str) -> bool {
    let Ok(key) = normalize_key(public_key) else {
        return false;
    };
    env_keys().contains(&key)
        || load().is_ok_and(|s| s.trusted_keys.iter().any(|k| k.public_key == key))
}

fn env_keys() -> Vec<String> {
    std::env::var(TRUST_ENV)
        .ok()
        .into_iter()
        .flat_map(|v| {
            v.split(',')
                .filter_map(|k| normalize_key(k).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Lower-case + trim a hex key and check it is a 32-byte (64-hex-char) Ed25519
/// public key. Rejecting malformed input here keeps comparisons exact.
fn normalize_key(key: &str) -> Result<String, String> {
    let k = key.trim().to_ascii_lowercase();
    if k.len() != 64 || !k.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "invalid public key (expected 64 hex chars, got {})",
            k.len()
        ));
    }
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::data_dir::isolated_data_dir;

    fn sample_key() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn normalize_rejects_bad_keys() {
        assert!(normalize_key("xyz").is_err());
        assert!(normalize_key(&"zz".repeat(32)).is_err());
        assert!(normalize_key(&sample_key()).is_ok());
    }

    #[test]
    fn env_override_is_trusted() {
        let _iso = isolated_data_dir();
        let key = sample_key();
        crate::test_env::set_var(TRUST_ENV, &key);
        assert!(is_trusted(&key));
        crate::test_env::remove_var(TRUST_ENV);
    }
}
