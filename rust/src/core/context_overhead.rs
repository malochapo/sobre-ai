//! Honest accounting of the fixed per-turn context sobre injects.
//!
//! Two components ride every request and — on a provider WITHOUT prompt caching
//! — are re-billed on every turn:
//!  - the exposed MCP **tool schemas** (description + input schema of each tool),
//!  - the MCP **server instructions** block — the only guidance sobre gives;
//!    it writes nothing into the host's own instruction files.
//!
//! `sobre gain` shows this figure next to the savings: the savings count
//! tokens removed from tool output, while this prefix is added to every
//! request. With the proxy in the path, the prefix is already inside the real
//! cost the provider reported.

use crate::core::tokens::count_tokens;

/// A measured breakdown, in tokens, of the per-turn context sobre adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextOverhead {
    /// Number of MCP tools exposed (the schema-bearing surface).
    pub tool_count: usize,
    /// Tokens for all exposed tool descriptions + input schemas.
    pub tool_schema_tokens: usize,
    /// Tokens for the MCP server instructions block.
    pub instruction_tokens: usize,
}

impl ContextOverhead {
    /// Total per-turn overhead in tokens.
    #[must_use]
    pub fn total_tokens(&self) -> usize {
        self.tool_schema_tokens + self.instruction_tokens
    }

    /// Measure the overhead for the currently-configured MCP surface. Uses the
    /// same advertisement policy as the live `tools/list` handler (the registry
    /// with description compression), so the number reflects what this install
    /// actually advertises.
    #[must_use]
    pub fn measure() -> Self {
        let tools = crate::server::tool_visibility::advertised_tool_defs_default();
        let tool_count = tools.len();
        let tool_schema_tokens = tools.iter().map(tool_tokens).sum();

        let instruction_tokens = count_tokens(&crate::instructions::build_instructions());

        Self {
            tool_count,
            tool_schema_tokens,
            instruction_tokens,
        }
    }
}

/// Description + input-schema tokens for one tool definition — exactly the two
/// fields a client re-sends in every request's tool list.
pub fn tool_tokens(t: &rmcp::model::Tool) -> usize {
    let desc = t
        .description
        .as_ref()
        .map_or(0, |d| count_tokens(d.as_ref()));
    let schema = count_tokens(&serde_json::to_string(&t.input_schema).unwrap_or_default());
    desc + schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_reports_nonzero_components() {
        // `isolated_data_dir` also holds the env lock, so a concurrent test
        // toggling the knobs can't perturb this.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let o = ContextOverhead::measure();
        assert!(o.tool_count > 0, "must expose at least one tool");
        assert!(o.tool_schema_tokens > 0, "tool schemas carry tokens");
        assert!(o.instruction_tokens > 0, "instructions carry tokens");
        assert_eq!(
            o.total_tokens(),
            o.tool_schema_tokens + o.instruction_tokens
        );
    }

    #[test]
    fn total_is_sum_of_parts() {
        let o = ContextOverhead {
            tool_count: 10,
            tool_schema_tokens: 100,
            instruction_tokens: 200,
        };
        assert_eq!(o.total_tokens(), 300);
    }

    #[test]
    fn per_turn_prefix_stays_within_budget() {
        // Ratchet on the fixed per-turn prefix: the 15 tool schemas and the MCP
        // instructions. Measured at 5109 tokens, 113 more than before every
        // action became an enum with its required parameters (so a host can
        // reject a malformed call before it runs); the budget leaves ~3% for
        // small wording changes. A tool or a paragraph more fails here, which
        // is the point — raise the budget only in the same commit that
        // justifies the growth.
        const PER_TURN_PREFIX_BUDGET_TOKENS: usize = 5260;

        let _iso = crate::core::data_dir::isolated_data_dir();
        let registered = crate::server::registry::build_registry().tool_defs().len();
        let o = ContextOverhead::measure();

        assert_eq!(
            o.tool_count, registered,
            "the advertised surface must be exactly the registry"
        );
        assert!(
            o.total_tokens() <= PER_TURN_PREFIX_BUDGET_TOKENS,
            "per-turn prefix = {} tok (schemas {} + instr {}), budget {}",
            o.total_tokens(),
            o.tool_schema_tokens,
            o.instruction_tokens,
            PER_TURN_PREFIX_BUDGET_TOKENS,
        );
    }
}
