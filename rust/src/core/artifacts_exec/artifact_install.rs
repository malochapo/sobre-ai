//! Unified artifact installer (GH #724/#725, Phase 1) — the one download →
//! verify → atomic-install path for every managed binary artifact lean-ctx
//! fetches: grammar dylibs (#690) and prebuilt addon binaries.
//!
//! Extracted from `grammar_install` (which keeps its zero-config policy
//! gates and now delegates the mechanics here) so the flow exists exactly
//! once: bounded-timeout fetch via
//! [`crate::core::http_client::ureq_agent_with_timeouts`], SHA-256 verify of
//! a sibling `.tmp` file via [`super::binhash::sha256_file`], hardened file
//! permissions + macOS ad-hoc signing, then an atomic rename — a bad
//! download never lands at the destination.
//!
//! Addon binaries install into the **managed bin dir**
//! `<data_dir>/addons/bin/<name>/<version>/` — deliberately never on `PATH`
//! and never a shared user-writable location. The gateway spawns them by
//! absolute path recorded in the install receipt, closing both PATH
//! hijacking and the "download a binary and move it around manually" UX.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::binhash::sha256_file;

/// One platform's downloadable artifact: a grammar dylib or an addon binary.
/// The shape every registry surface shares (GH #724 — `GrammarAsset` is an
/// alias of this since Phase 1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactAsset {
    /// Release asset filename, e.g. `lean-md-aarch64-apple-darwin` or
    /// `lua-x86_64-pc-windows-msvc.dll`.
    pub filename: String,
    /// Download URL for this asset.
    pub url: String,
    /// SHA-256 of the artifact bytes (hex). Mandatory: unpinned artifacts
    /// are refused before any network I/O.
    pub sha256: String,
}

/// How the installed artifact will be used — decides on-disk hardening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactUse {
    /// dlopen'd into our own process (grammar dylib): read-only (0o444).
    InProcessDylib,
    /// Spawned as a subprocess (addon binary): read + execute (0o555).
    Executable,
}

/// Rust target-triple key this build was compiled for — matches the asset
/// keys release CI matrices publish under (grammar dylibs and addon
/// binaries use the same convention).
pub fn current_target_triple() -> &'static str {
    if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_arch = "aarch64", target_os = "windows")) {
        "aarch64-pc-windows-msvc"
    } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
        "aarch64-unknown-linux-gnu"
    } else {
        "unknown"
    }
}

/// The managed install dir for one addon version:
/// `<data_dir>/addons/bin/<name>/<version>/`. A blank version (local
/// manifests without one) maps to `unversioned` so the layout stays uniform.
pub fn managed_bin_dir(name: &str, version: &str) -> Result<PathBuf, String> {
    let version = version.trim();
    Ok(crate::core::data_dir::lean_ctx_data_dir()?
        .join("addons")
        .join("bin")
        .join(name)
        .join(if version.is_empty() {
            "unversioned"
        } else {
            version
        }))
}

/// Reject an artifact missing its mandatory SHA-256 pin before any policy or network work.
pub(crate) fn require_sha256_pin(context: &str, expected_sha256: &str) -> Result<(), String> {
    if expected_sha256.trim().is_empty() {
        return Err(format!(
            "{context} asset has no sha256 pin — refusing to fetch"
        ));
    }
    Ok(())
}

/// Download `url`, verify its SHA-256, harden permissions and atomically
/// move it to `dest`. `context` prefixes every error/log line (e.g.
/// ``grammar `lua` `` or ``addon `lean-md` ``) so callers keep their
/// established message shapes.
pub(crate) fn fetch_verified(
    context: &str,
    url: &str,
    expected_sha256: &str,
    dest: &Path,
    usage: ArtifactUse,
) -> Result<(), String> {
    require_sha256_pin(context, expected_sha256)?;

    let agent = crate::core::http_client::ureq_agent_with_timeouts(
        Some(std::time::Duration::from_secs(10)),
        Some(std::time::Duration::from_secs(15)),
        Some(std::time::Duration::from_secs(20)),
    );
    let response = agent
        .get(url)
        .header(
            "User-Agent",
            &format!("lean-ctx/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| format!("{context} fetch failed: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{context} download read failed: {e}"))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;

    let actual = sha256_file(&tmp)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{context} download hash mismatch: expected {expected_sha256}, got {actual} — \
             refusing to install"
        ));
    }

    // Hardening: a dylib is dlopen'd into our own process → read-only on
    // disk (a deliberate swap still fails the per-load hash pin). An addon
    // binary is spawned → read + execute, still not writable (a swap fails
    // the binhash spawn pin). macOS ad-hoc signing keeps a copy that picked
    // up a quarantine xattr along the way loadable/runnable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = match usage {
            ArtifactUse::InProcessDylib => 0o444,
            ArtifactUse::Executable => 0o555,
        };
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = &usage;
    #[cfg(target_os = "macos")]
    crate::core::codesign::adhoc_sign(&tmp);

    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_pin_is_refused_without_network() {
        let dest = std::env::temp_dir().join(format!("lc-artifact-test-{}-a", std::process::id()));
        let err = fetch_verified(
            "addon `x`",
            "https://example.invalid/bin",
            "",
            &dest,
            ArtifactUse::Executable,
        )
        .unwrap_err();
        assert!(err.contains("sha256 pin"), "got: {err}");
        assert!(!dest.exists());
    }

    #[test]
    fn current_triple_is_known_on_ci_platforms() {
        if cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        )) && cfg!(any(target_arch = "x86_64", target_arch = "aarch64"))
        {
            assert_ne!(current_target_triple(), "unknown");
        }
    }
}
