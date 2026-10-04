// Machine-verified contract versions.
pub const MCP_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const CONTEXT_IR_V1_SCHEMA_VERSION: u32 = 1;
pub const CCP_SESSION_BUNDLE_V1_SCHEMA_VERSION: u32 = 1;
pub const GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION: u32 = 1;
pub const CONTEXT_SNAPSHOT_V1_SCHEMA_VERSION: u32 = 1;

/// Stability classification of a contract document (GL #394).
///
/// The classification is normative — `tests/suite/contracts_frozen.rs`
/// enforces it:
/// * `Frozen` — the normative surface is immutable. Any change to the doc file
///   fails CI; semantic evolution requires a new `-v2.md` file (the v1 file
///   stays in place for existing integrations).
/// * `Stable` — additive evolution allowed (new optional fields, new sections);
///   breaking changes require a version bump (a new `-vN.md` file).
/// * `Experimental` — may change or disappear without notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    Frozen,
    Stable,
    Experimental,
}

impl ContractStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ContractStatus::Frozen => "frozen",
            ContractStatus::Stable => "stable",
            ContractStatus::Experimental => "experimental",
        }
    }
}

/// One contract document under `docs/contracts/`.
pub struct ContractDoc {
    /// Short stable identifier.
    pub id: &'static str,
    /// File name inside `docs/contracts/` (the normative artifact).
    pub doc_file: &'static str,
    pub version: u32,
    pub status: ContractStatus,
}

/// The complete classified inventory of `docs/contracts/*.md`.
/// `tests/suite/contracts_frozen.rs` asserts that every file in the directory
/// is listed here (no contract can stay unclassified) and that frozen docs
/// never change.
pub fn contract_docs() -> Vec<ContractDoc> {
    use ContractStatus::{Experimental, Frozen, Stable};
    let doc = |id, doc_file, version, status| ContractDoc {
        id,
        doc_file,
        version,
        status,
    };
    vec![
        // ── Frozen: externally consumed format ─────────────────────────────
        doc("context-ir", "context-ir-v1.md", 1, Frozen),
        // ── Stable: additive evolution allowed ──────────────────────────────
        doc("ccp-session-bundle", "ccp-session-bundle-v1.md", 1, Stable),
        doc("conformance", "conformance-v1.md", 1, Stable),
        doc("extractors", "extractors-v1.md", 1, Stable),
        doc(
            "gotchas-reminders",
            "gotchas-reminders-contract-v1.md",
            1,
            Stable,
        ),
        doc(
            "graph-reproducibility",
            "graph-reproducibility-contract-v1.md",
            1,
            Stable,
        ),
        doc(
            "knowledge-policy",
            "knowledge-policy-contract-v1.md",
            1,
            Stable,
        ),
        doc(
            "memory-boundary",
            "memory-boundary-contract-v1.md",
            1,
            Stable,
        ),
        // ── Experimental: may change without notice ─────────────────────────
        // Quality loop (GL #494): edit-failure feedback into mode selection.
        doc("quality-loop", "quality-loop-v1.md", 1, Experimental),
        // Context Time Machine (GL #1022/#1023): git-anchored, signed temporal
        // snapshot format — evolving additively until stable.
        doc(
            "context-snapshot",
            "context-snapshot-v1.md",
            1,
            Experimental,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_docs_have_unique_ids_and_files() {
        let docs = contract_docs();
        let mut ids: Vec<_> = docs.iter().map(|d| d.id).collect();
        let mut files: Vec<_> = docs.iter().map(|d| d.doc_file).collect();
        ids.sort_unstable();
        files.sort_unstable();
        let unique_ids: std::collections::BTreeSet<_> = ids.iter().collect();
        let unique_files: std::collections::BTreeSet<_> = files.iter().collect();
        assert_eq!(unique_ids.len(), docs.len(), "duplicate contract id");
        assert_eq!(unique_files.len(), docs.len(), "duplicate doc file");
    }

    #[test]
    fn frozen_set_covers_the_platform_promises() {
        // The freeze (GL #394) is only meaningful if the externally consumed
        // surfaces are actually in it. Removing one of these from `Frozen`
        // is itself a breaking policy change.
        let docs = contract_docs();
        for id in ["context-ir"] {
            let entry = docs.iter().find(|d| d.id == id).expect("listed");
            assert_eq!(
                entry.status,
                ContractStatus::Frozen,
                "{id} must stay frozen"
            );
        }
    }

    #[test]
    fn doc_files_follow_versioned_naming() {
        // v1→v2 rule: every doc file carries its version suffix so a breaking
        // change lands as a NEW file instead of mutating the old one.
        for d in contract_docs() {
            assert!(
                d.doc_file.ends_with(&format!("-v{}.md", d.version)),
                "{} must end in -v{}.md",
                d.doc_file,
                d.version
            );
        }
    }
}
