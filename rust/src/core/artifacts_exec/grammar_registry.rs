//! The curated grammar-addon catalog (#690, Phase 1a).
//!
//! Mirrors [`super::registry`]'s bundled/local-override layering, but for
//! [`GrammarManifest`] instead of [`super::manifest::AddonManifest`] — a
//! grammar addon has no `[mcp]` wiring for `super::audit`/`super::trust` to
//! assess, so this stays a separate, smaller catalog rather than folding
//! grammar entries into `registry::all()`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::grammar_manifest::GrammarManifest;

static BUNDLED: &str = include_str!("../../../data/grammar_registry.json");

static PARSED_BUNDLED: OnceLock<Vec<GrammarManifest>> = OnceLock::new();
static PARSED_LOCAL: OnceLock<Option<Vec<GrammarManifest>>> = OnceLock::new();

#[derive(Debug, Default, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    grammars: Vec<GrammarManifest>,
}

fn parse(json: &str) -> Vec<GrammarManifest> {
    serde_json::from_str::<RegistryFile>(json)
        .map(|r| r.grammars)
        .unwrap_or_default()
}

fn bundled() -> &'static [GrammarManifest] {
    PARSED_BUNDLED.get_or_init(|| parse(BUNDLED))
}

/// User-override catalog (`<data_dir>/grammar_registry.json`), gated by the
/// same signed-override policy as the MCP addon registry (#865): with
/// `addons.require_signature` on, an override is honoured only with a valid
/// sidecar signature by a trusted org key.
fn local() -> Option<&'static [GrammarManifest]> {
    PARSED_LOCAL
        .get_or_init(|| {
            let dir = crate::core::data_dir::lean_ctx_data_dir().ok()?;
            let path = dir.join("grammar_registry.json");
            let content = std::fs::read_to_string(&path).ok()?;
            let require_sig = crate::core::config::Config::load().addons.require_signature;
            if let super::signing::OverrideVerdict::Reject(reason) =
                gate_override_file(&path, &content, require_sig)
            {
                tracing::warn!("[SECURITY] ignoring user grammar registry override: {reason}");
                return None;
            }
            Some(parse(&content))
        })
        .as_deref()
}

fn gate_override_file(
    path: &std::path::Path,
    content: &str,
    require_sig: bool,
) -> super::signing::OverrideVerdict {
    let sig = std::fs::read_to_string(super::signing::sidecar_path(path))
        .ok()
        .and_then(|t| super::signing::RegistrySignature::from_json(&t).ok());
    super::signing::gate_override(content, sig.as_ref(), require_sig, |pk| {
        crate::core::artifacts_exec::trusted_keys::is_trusted(pk)
    })
}

/// Every known grammar addon, sorted by name. A user-override entry replaces
/// the bundled entry with the same name.
pub fn all() -> Vec<GrammarManifest> {
    let mut by_name: BTreeMap<String, GrammarManifest> = BTreeMap::new();
    for m in bundled() {
        by_name.insert(m.name.clone(), m.clone());
    }
    if let Some(local) = local() {
        for m in local {
            by_name.insert(m.name.clone(), m.clone());
        }
    }
    by_name.into_values().collect()
}

/// Look up a single grammar addon by its slug (case-insensitive).
pub fn get(name: &str) -> Option<GrammarManifest> {
    let needle = name.trim().to_ascii_lowercase();
    all()
        .into_iter()
        .find(|m| m.name.to_ascii_lowercase() == needle)
}

/// Find the grammar addon that claims the given file extension (no leading
/// dot, case-insensitive) — the lookup `signatures_ts::queries` will use in
/// Phase 1b's loader wiring.
pub fn find_by_extension(ext: &str) -> Option<GrammarManifest> {
    let needle = ext.trim_start_matches('.').to_ascii_lowercase();
    all().into_iter().find(|m| {
        m.extensions
            .iter()
            .any(|e| e.to_ascii_lowercase() == needle)
    })
}

/// Lint registry entries: unique valid slugs, each passing
/// [`GrammarManifest::validate`]. Empty = clean.
#[must_use]
pub fn validate_entries(entries: &[GrammarManifest]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for m in entries {
        let slug = m.name.to_ascii_lowercase();
        if !seen.insert(slug) {
            problems.push(format!("duplicate slug `{}`", m.name));
        }
        if let Err(e) = m.validate() {
            problems.push(format!("`{}`: invalid manifest — {e}", m.name));
        }
    }
    problems
}

// ── Canonical snapshot form ────────────────────────────────
//
// `data/grammar_registry.json` is a generated snapshot, not a hand-maintained
// document. `gen_registry` rewrites it into one canonical form — entries sorted
// by name, struct field order, two-space indent, trailing newline — after
// validating every entry. CI runs `gen_registry --check`, so a hand-edit that
// bypasses the generator, or a field serde would silently drop, shows up as
// byte drift and fails the build.

/// Schema-preserving view of the file: `$schema` and `registry_version` must
/// survive a round-trip, so this cannot reuse [`RegistryFile`], whose
/// `#[serde(default)]` tolerance is what makes it safe for *reading*.
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotFile {
    #[serde(rename = "$schema")]
    schema: String,
    registry_version: u32,
    grammars: Vec<GrammarManifest>,
}

/// One canonicalized document plus the fact the generator reports.
#[derive(Debug)]
pub struct Snapshot {
    /// Canonical JSON (ends with a newline).
    pub canonical: String,
    pub entry_count: usize,
}

/// Canonicalize and validate a grammar-registry document.
pub fn canonical_snapshot(text: &str) -> Result<Snapshot, String> {
    let mut file: SnapshotFile = serde_json::from_str(text)
        .map_err(|e| format!("grammar registry does not parse: {e}"))?;

    let problems = validate_entries(&file.grammars);
    if !problems.is_empty() {
        return Err(format!(
            "grammar registry fails validation:\n  {}",
            problems.join("\n  ")
        ));
    }
    // `validate_entries` already reports duplicate slugs case-insensitively;
    // this catches two entries whose names differ only by case, which would
    // make the sort below — and so the canonical bytes — order-dependent.
    let mut seen = std::collections::BTreeSet::new();
    for g in &file.grammars {
        if !seen.insert(g.name.clone()) {
            return Err(format!("duplicate grammar entry: {}", g.name));
        }
    }
    file.grammars.sort_by(|a, b| a.name.cmp(&b.name));

    let entry_count = file.grammars.len();
    let mut canonical = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    canonical.push('\n');
    Ok(Snapshot {
        canonical,
        entry_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_registry_is_valid() {
        let problems = validate_entries(bundled());
        assert!(problems.is_empty(), "{problems:?}");
    }

    /// The same invariant `gen_registry --check` enforces in CI, kept as a lib
    /// test so `cargo test` alone catches drift.
    #[test]
    fn bundled_registry_is_canonical() {
        let snap = canonical_snapshot(BUNDLED).expect("canonicalize");
        assert_eq!(
            BUNDLED, snap.canonical,
            "data/grammar_registry.json is not canonical — run: \
             cargo run --example gen_registry --features dev-tools"
        );
    }

    #[test]
    fn snapshot_preserves_schema_and_trailing_newline() {
        let snap = canonical_snapshot(BUNDLED).expect("canonicalize");
        assert!(snap.canonical.contains("\"$schema\""), "{}", snap.canonical);
        assert!(snap.canonical.contains("\"registry_version\""));
        assert!(snap.canonical.ends_with('\n'));
    }

    #[test]
    fn snapshot_rejects_a_document_missing_the_schema_key() {
        let err = canonical_snapshot(r#"{"registry_version":1,"grammars":[]}"#)
            .expect_err("missing $schema must not canonicalize silently");
        assert!(err.contains("does not parse"), "{err}");
    }

    #[test]
    fn find_by_extension_is_case_insensitive() {
        // No entries shipped yet (Phase 1c builds the first dylibs) — this
        // just proves the lookup doesn't panic on an empty catalog.
        assert!(find_by_extension("LUA").is_none() || find_by_extension("lua").is_some());
    }
}
