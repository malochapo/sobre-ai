//! # Terse Compression Engine
//!
//! Unified, 4-layer token compression pipeline.
//!
//! ## Architecture
//!
//! ```text
//! Tool Output → [Layer 1: Deterministic] → [Layer 2: Residual] → Compressed Output
//! MCP Tools List → [Layer 4: Description Terse] → Compact Descriptions
//! ```
//!
//! ## Layers
//!
//! - **Layer 1** (`engine.rs`): Deterministic output compression — surprisal scoring,
//!   content/function word filtering, domain dictionaries, quality gate.
//! - **Layer 2** (`residual.rs`): Pattern-aware post-terse — applies after pattern
//!   compression, avoids double-compression, tracks attribution.
//! - **Layer 4** (`mcp_compress.rs`): MCP description compression — shrinks tool
//!   descriptions, lazy-load stubs, on-demand expansion.

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
