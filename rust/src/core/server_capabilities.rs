//! `GET /v1/capabilities` — runtime discovery of what this lean-ctx instance
//! supports, so any client (any language) can branch on real features instead
//! of trial calls. It was served over HTTP by a route that is gone; the
//! payload builder stays because `doctor` and the drift tests read it.
//!
//! Contract: `docs/contracts/capabilities-contract-v1.md`. The set of
//! [`TOP_LEVEL_KEYS`] is the stable contract surface and is bound to that doc
//! by `tests/capabilities_contract_up_to_date.rs`.
//!
//! Not to be confused with [`crate::core::capabilities`], which models RBAC
//! permissions (`fs:read`, …). This module describes *server* capabilities.

use serde_json::{Value, json};

use crate::core::contracts::{CAPABILITIES_CONTRACT_VERSION, status_kv, versions_kv};

/// Stable, documented top-level keys of the capabilities document.
pub const TOP_LEVEL_KEYS: [&str; 10] = [
    "contract_version",
    "server",
    "plane",
    "transports",
    "read_modes",
    "tools",
    "features",
    "extensions",
    "contracts",
    "contract_status",
];

/// Build the capabilities document for this running instance.
pub fn capabilities_value() -> Value {
    let manifest = crate::core::mcp_manifest::manifest_value();
    let tool_names = tool_names(&manifest);
    let read_modes = manifest.get("read_modes").cloned().unwrap_or(Value::Null);
    json!({
        "contract_version": CAPABILITIES_CONTRACT_VERSION,
        "server": {
            "name": "lean-ctx",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "plane": "personal",
        "transports": ["stdio-mcp", "http-mcp", "rest", "sse"],
        "read_modes": read_modes,
        "tools": {
            "total": tool_names.len(),
            "names": tool_names,
        },
        "features": features(),
        "extensions": extensions(),
        "contracts": versions_kv(),
        // Stability per contract document (frozen|stable|experimental) so
        // clients can check compatibility before building against a surface
        // (GL #394). Additive: existing consumers are unaffected.
        "contract_status": status_kv(),
    })
}

fn tool_names(manifest: &Value) -> Vec<String> {
    manifest
        .get("tools")
        .and_then(|t| t.get("granular"))
        .and_then(|g| g.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Always-on capabilities plus compiled-in feature flags. Booleans reflect what
/// this binary can actually do.
fn features() -> Value {
    json!({
        "compression": true,
        "caching": true,
        "knowledge": true,
        "session": true,
        "gateway": true,
        "sensitivity_floor": true,
        "savings_ledger": true,
        "audit_trail": true,
        "routing": true,
        "ast_compression": cfg!(feature = "tree-sitter"),
        "semantic_search": cfg!(feature = "embeddings"),
        "wasm_runtime": cfg!(feature = "wasm"),
        "shape_translation": cfg!(feature = "shape-xlat"),
    })
}

/// Runtime-discovered extensions: the registered
/// read-modes / compressors / chunkers (EPIC 12.9). The sandboxed extension
/// runtime (EPIC 12.8) expands what registers here.
fn extensions() -> Value {
    let (read_modes, compressors, chunkers) = crate::core::extension_registry::global()
        .read()
        .map(|r| (r.read_mode_names(), r.compressor_names(), r.chunker_names()))
        .unwrap_or_default();

    json!({
        "read_modes": read_modes,
        "compressors": compressors,
        "chunkers": chunkers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_has_exactly_documented_top_level_keys() {
        let v = capabilities_value();
        let obj = v.as_object().expect("capabilities is an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected: Vec<&str> = TOP_LEVEL_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected, "top-level keys drifted from TOP_LEVEL_KEYS");
    }

    #[test]
    fn contract_version_matches_constant() {
        let v = capabilities_value();
        assert_eq!(v["contract_version"], json!(CAPABILITIES_CONTRACT_VERSION));
    }

    #[test]
    fn lists_real_tools_and_read_modes() {
        let v = capabilities_value();
        assert!(
            v["tools"]["total"].as_u64().unwrap_or(0) > 0,
            "expected at least one tool"
        );
        assert!(v["read_modes"]["modes"].is_array());
    }

    #[test]
    fn extensions_expose_registry_builtins() {
        let v = capabilities_value();
        let ext = &v["extensions"];
        let compressors = ext["compressors"].as_array().expect("compressors array");
        assert!(compressors.iter().any(|c| c == "identity"));
        assert!(
            ext["read_modes"]
                .as_array()
                .is_some_and(|a| a.iter().any(|m| m == "full"))
        );
        assert!(
            ext["chunkers"]
                .as_array()
                .is_some_and(|a| a.iter().any(|c| c == "lines"))
        );
    }

    #[test]
    fn reports_compiled_features() {
        let v = capabilities_value();
        // Always-on capabilities are unconditionally true.
        assert_eq!(v["features"]["compression"], json!(true));
        assert_eq!(v["features"]["savings_ledger"], json!(true));
        // Feature-gated flags mirror the compile-time cfg.
        assert_eq!(
            v["features"]["semantic_search"],
            json!(cfg!(feature = "embeddings"))
        );
    }
}
