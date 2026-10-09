//! Layer 2: Pattern-aware residual compression.
//!
//! Runs AFTER pattern compression (95+ command-specific patterns) to apply
//! terse compression only on the residual text that patterns didn't touch.

use super::counter;
use super::engine;
use crate::core::config::CompressionLevel;

/// Result of residual compression.
#[derive(Debug)]
pub struct ResidualResult {
    pub output: String,
    pub total_savings_pct: f32,
    pub quality_passed: bool,
}

/// Applies Layer 2 residual compression: terse on top of the output of
/// pattern compression.
pub fn compress_residual(
    original_text: &str,
    pattern_compressed: &str,
    level: &CompressionLevel,
) -> ResidualResult {
    let tokens_before = counter::count(original_text);

    if !level.is_active() {
        return ResidualResult {
            output: pattern_compressed.to_string(),
            total_savings_pct: counter::savings_pct(
                tokens_before,
                counter::count(pattern_compressed),
            ),
            quality_passed: true,
        };
    }

    let engine_result = engine::compress(pattern_compressed, level);
    ResidualResult {
        total_savings_pct: counter::savings_pct(tokens_before, engine_result.tokens_after),
        output: engine_result.output,
        quality_passed: engine_result.quality_passed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residual_off_passes_through() {
        let result = compress_residual("hello world", "hello world", &CompressionLevel::Off);
        assert_eq!(result.output, "hello world");
    }
}
