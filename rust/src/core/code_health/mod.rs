//! Native code-health engine — clean code as a token-cost lever.
//!
//! Computes the structural signals the Sonar study links to agent token cost —
//! **cognitive complexity** (S3776-style) and **naming quality** — once during
//! indexing, then fans them out across the data fabric
//! and the agent surfaces.
//!
//! Naming note: this is distinct from [`crate::core::quality`] (compression
//! fidelity); this module scores the *source code's* navigability.

pub mod annotate;
#[cfg(feature = "tree-sitter")]
pub(crate) mod astutil;
pub mod cognitive;
pub mod delta;
pub mod fabric;
pub mod gate;
pub mod naming;
pub mod persist;
pub mod scan;
pub mod score;

pub use cognitive::{FunctionCognitive, cognitive_per_function};
pub use delta::{cognitive_delta, format_gate_notice, worst_regression};
pub use naming::{NamingFinding, naming_findings};
pub use score::{Hotspot, NavigabilityInputs, NavigabilityScore, navigability};

use serde::Serialize;

/// Cognitive-complexity threshold above which a function is a hotspot.
pub const COGNITIVE_THRESHOLD: u32 = 15;

/// Combined per-file health: cognitive scores plus naming findings. This is the
/// single entry point used by the edit-gate and read annotations.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FileHealth {
    pub functions: Vec<FunctionCognitive>,
    pub naming: Vec<NamingFinding>,
}

impl FileHealth {
    /// The single worst cognitive complexity in the file (0 if none).
    pub fn worst_cognitive(&self) -> u32 {
        self.functions
            .iter()
            .map(|f| f.cognitive)
            .max()
            .unwrap_or(0)
    }
}

/// Analyze one file's `source` for the given file `extension`.
///
/// Returns `None` only when tree-sitter is disabled or the extension is
/// unsupported (i.e. no functions could be parsed). Naming findings default to
/// empty when the language has no analyzable identifiers.
pub fn analyze_file(source: &str, extension: &str) -> Option<FileHealth> {
    let functions = cognitive_per_function(source, extension)?;
    let naming = naming_findings(source, extension).unwrap_or_default();
    Some(FileHealth { functions, naming })
}

#[cfg(all(test, feature = "tree-sitter"))]
mod tests {
    use super::*;

    #[test]
    fn analyze_file_combines_signals() {
        let src = "fn _xfm_q2(a: bool, b: bool) { if a { if b {} } }\n";
        let health = analyze_file(src, "rs").unwrap();
        assert_eq!(health.functions.len(), 1);
        assert_eq!(health.worst_cognitive(), 3);
        assert_eq!(health.naming.len(), 1, "cryptic name flagged");
    }

    #[test]
    fn analyze_file_unsupported_ext_is_none() {
        assert!(analyze_file("plain text", "txt").is_none());
    }
}
