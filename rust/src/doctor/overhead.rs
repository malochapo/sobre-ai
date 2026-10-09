//! `sobre doctor overhead` — honest fixed-cost accounting.
//!
//! Shows what a session costs BEFORE sobre saves anything:
//!  1. the advertised MCP tool schemas (what `tools/list` returns),
//!  2. the MCP server instructions block.
//!
//! Research context: fixed context costs both money and model attention
//! (context degradation starts well below typical window limits), so every
//! always-on token has to justify itself.

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RST: &str = "\x1b[0m";

#[derive(Debug, serde::Serialize)]
pub(super) struct OverheadReport {
    pub tool_count: usize,
    pub tool_schema_tokens: usize,
    pub instruction_tokens: usize,
    /// The per-session budget.
    pub budget_tokens: usize,
    /// Whether `total_tokens` exceeds `budget_tokens`.
    pub over_budget: bool,
}

impl OverheadReport {
    fn total_tokens(&self) -> usize {
        self.tool_schema_tokens + self.instruction_tokens
    }

    /// Strictly above the budget.
    fn is_over_budget(&self) -> bool {
        self.total_tokens() > self.budget_tokens
    }
}

/// The same measurement `sobre gain` reports, so the two never disagree.
#[must_use]
pub(super) fn measure() -> OverheadReport {
    let measured = crate::core::context_overhead::ContextOverhead::measure();
    let budget_tokens = crate::core::config::CONTEXT_BUDGET_TOKENS;

    let mut report = OverheadReport {
        tool_count: measured.tool_count,
        tool_schema_tokens: measured.tool_schema_tokens,
        instruction_tokens: measured.instruction_tokens,
        budget_tokens,
        over_budget: false,
    };
    report.over_budget = report.is_over_budget();
    report
}

pub(super) fn run_overhead(json: bool, gate: bool) -> i32 {
    let report = measure();

    // `--gate` turns a budget breach into a non-zero exit for CI; without
    // it the report is purely informational regardless of the breach.
    let exit_code = i32::from(gate && report.over_budget);

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => crate::outln!("{s}"),
            Err(e) => {
                crate::errln!("doctor overhead: JSON serialization failed: {e}");
                return 2;
            }
        }
        return exit_code;
    }

    crate::outln!("{BOLD}Fixed context overhead per session{RST}");
    crate::outln!("{DIM}What every session pays before any compression saves a token.{RST}\n");

    // 1. Tool schemas
    crate::outln!(
        "  {BOLD}MCP tool schemas{RST}      {:>6} tok  {DIM}({} tools advertised){RST}",
        report.tool_schema_tokens,
        report.tool_count
    );

    // 2. Instructions
    crate::outln!(
        "  {BOLD}MCP instructions{RST}      {:>6} tok",
        report.instruction_tokens
    );

    crate::outln!();
    let total = report.total_tokens();
    let color = if report.over_budget { YELLOW } else { GREEN };
    crate::outln!("  {BOLD}Total fixed cost{RST}      {color}{total:>6} tok / session{RST}");
    if report.over_budget {
        // Machine-readable so CI/log scrapers can key on a stable token.
        crate::outln!(
            "  {YELLOW}⚠ OVER_BUDGET: {total} tok > budget {} tok {RST}",
            report.budget_tokens
        );
    } else {
        crate::outln!(
            "  {DIM}Within budget ({} / {} tok).{RST}",
            total,
            report.budget_tokens
        );
    }
    crate::outln!(
        "  {DIM}Every turn re-bills it; prompt caching makes that cheaper while the prefix stays byte-stable, at a rate that depends on the provider.{RST}"
    );

    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(schema: usize, instr: usize, budget: usize) -> OverheadReport {
        OverheadReport {
            tool_count: 1,
            tool_schema_tokens: schema,
            instruction_tokens: instr,
            budget_tokens: budget,
            over_budget: false,
        }
    }

    #[test]
    fn over_budget_threshold_is_strict() {
        assert!(
            report(150, 200, 300).is_over_budget(),
            "350 > 300 must breach"
        );
        assert!(
            !report(150, 200, 350).is_over_budget(),
            "a total equal to the budget is within it"
        );
        assert!(
            !report(150, 200, 8000).is_over_budget(),
            "350 < 8000 is within budget"
        );
    }
}
