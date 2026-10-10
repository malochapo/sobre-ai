//! Cognitive complexity of the source code, computed on demand from the
//! file being read, searched or edited: the edit gate on `sobre_patch`, the
//! `cc=` annotations in `sobre_read`, and the complexity shown with a
//! symbol. Nothing is stored.
//!
//! Naming note: this is distinct from [`crate::core::quality`] (compression
//! fidelity); this module scores the *source code's* navigability.

pub mod annotate;
#[cfg(feature = "tree-sitter")]
pub(crate) mod astutil;
pub mod cognitive;
pub mod delta;
pub mod gate;

pub use delta::{cognitive_delta, format_gate_notice, worst_regression};

/// Cognitive-complexity threshold above which a function is a hotspot.
pub const COGNITIVE_THRESHOLD: u32 = 15;
