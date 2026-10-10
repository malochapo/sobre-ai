//! # Terse compression
//!
//! The generic, `compression_level`-driven compression of text no
//! command-specific pattern owns.
//!
//! ```text
//! Tool output    → [engine] ──────────────→ compressed output
//! Pattern output → [residual → engine] ───→ compressed output
//! Tool descriptions → [mcp_compress] ─────→ shorter descriptions
//! ```
//!
//! - `pipeline.rs`: the entry point; compacts pure JSON losslessly, keeps
//!   fenced code byte-exact, and runs the engine on the rest.
//! - `engine.rs`: line scoring (entropy, structural markers, repetition),
//!   decoration and filler removal, domain dictionaries, and a quality gate
//!   that keeps the original when paths or identifiers would be lost.
//! - `residual.rs`: the engine run on a pattern compressor's output.
//! - `mcp_compress.rs`: shorter tool descriptions at higher levels.

pub mod auto_dict;
pub mod counter;
pub mod dictionaries;
pub mod engine;
pub mod mcp_compress;
pub mod pipeline;
pub mod quality;
pub mod residual;
pub mod scoring;

/// Result of a compression pipeline run with full attribution.
#[derive(Debug, Clone)]
pub struct TerseResult {
    pub output: String,
    pub savings_pct: f32,
    pub quality_passed: bool,
}

impl TerseResult {
    pub fn passthrough(text: String) -> Self {
        Self {
            output: text,
            savings_pct: 0.0,
            quality_passed: true,
        }
    }
}
