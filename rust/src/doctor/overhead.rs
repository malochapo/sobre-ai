//! `lean-ctx doctor overhead` — honest fixed-cost accounting (#572).
//!
//! Shows what a session costs BEFORE lean-ctx saves anything:
//!  1. advertised MCP tool schemas (mirrors the live `tools/list` policy),
//!  2. the MCP server instructions block — sobre's only guidance channel,
//!  3. the wakeup briefing.
//!
//! Research context: fixed context costs both money and model attention
//! (context degradation starts well below typical window limits), so every
//! always-on token has to justify itself.

use std::path::PathBuf;

use crate::core::context_overhead::tool_tokens;
use crate::core::tokens::count_tokens;

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
    /// Tokens of the wakeup briefing (knowledge + session memory), delivered
    /// once per session via the AUTO CONTEXT block of the first tool call other
    /// than the `ctx_compose` overview (instructions stay byte-stable, #498);
    /// its own source, not folded into `instruction_tokens` (#964).
    pub wakeup_tokens: usize,
    /// The per-session budget (#964); 0 disables the check.
    pub budget_tokens: usize,
    /// Whether `total_tokens` exceeds a non-zero `budget_tokens` (#964).
    pub over_budget: bool,
}

impl OverheadReport {
    fn total_tokens(&self) -> usize {
        self.tool_schema_tokens + self.instruction_tokens + self.wakeup_tokens
    }

    /// Strictly above a non-zero budget; a budget of 0 disables the check (#964).
    fn is_over_budget(&self) -> bool {
        self.budget_tokens > 0 && self.total_tokens() > self.budget_tokens
    }
}

#[must_use]
pub(super) fn measure(project: &std::path::Path) -> OverheadReport {
    let advertised = crate::server::tool_visibility::advertised_tool_defs_default();
    let instructions = crate::instructions::build_instructions();

    // The wakeup briefing rides session start as its own source (#964). It reads
    // live knowledge + session state, so it reflects what this install actually
    // re-injects rather than a static estimate.
    let wakeup =
        crate::tools::impl_overview::build_wakeup_briefing(&project.to_string_lossy(), None);

    let budget_tokens = crate::core::config::CONTEXT_BUDGET_TOKENS;

    let mut report = OverheadReport {
        tool_count: advertised.len(),
        tool_schema_tokens: advertised.iter().map(tool_tokens).sum(),
        instruction_tokens: count_tokens(&instructions),
        wakeup_tokens: count_tokens(&wakeup),
        budget_tokens,
        over_budget: false,
    };
    report.over_budget = report.is_over_budget();
    report
}

pub(super) fn run_overhead(json: bool, gate: bool) -> i32 {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let project = std::env::current_dir().unwrap_or_else(|_| home.clone());
    let report = measure(&project);

    // `--gate` turns a budget breach into a non-zero exit for CI (#964); without
    // it the report is purely informational regardless of the breach.
    let exit_code = i32::from(gate && report.over_budget);

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("doctor overhead: JSON serialization failed: {e}");
                return 2;
            }
        }
        return exit_code;
    }

    println!("{BOLD}Fixed context overhead per session{RST}");
    println!("{DIM}What every session pays before any compression saves a token.{RST}\n");

    // 1. Tool schemas
    println!(
        "  {BOLD}MCP tool schemas{RST}      {:>6} tok  {DIM}({} tools advertised){RST}",
        report.tool_schema_tokens, report.tool_count
    );

    // 2. Instructions
    println!(
        "  {BOLD}MCP instructions{RST}      {:>6} tok",
        report.instruction_tokens
    );

    // 3. Wakeup briefing (knowledge + session memory re-injected on session start)
    println!(
        "  {BOLD}Wakeup briefing{RST}       {:>6} tok  {DIM}(knowledge + session memory){RST}",
        report.wakeup_tokens
    );

    println!();
    let total = report.total_tokens();
    let color = if report.over_budget { YELLOW } else { GREEN };
    println!("  {BOLD}Total fixed cost{RST}      {color}{total:>6} tok / session{RST}");
    if report.budget_tokens > 0 {
        if report.over_budget {
            // Machine-readable so CI/log scrapers can key on a stable token (#964).
            println!(
                "  {YELLOW}⚠ OVER_BUDGET: {total} tok > budget {} tok — trim the knowledge briefing.{RST}",
                report.budget_tokens
            );
        } else {
            println!(
                "  {DIM}Within budget ({} / {} tok).{RST}",
                total, report.budget_tokens
            );
        }
    }
    println!(
        "  {DIM}With provider prompt caching, repeated turns re-bill this at ~10% — but only if the prefix stays byte-stable.{RST}"
    );

    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(schema: usize, instr: usize, wakeup: usize, budget: usize) -> OverheadReport {
        OverheadReport {
            tool_count: 1,
            tool_schema_tokens: schema,
            instruction_tokens: instr,
            wakeup_tokens: wakeup,
            budget_tokens: budget,
            over_budget: false,
        }
    }

    #[test]
    fn total_includes_wakeup_source() {
        // #964: the wakeup briefing is its own fourth source in the total.
        let r = report(100, 200, 50, 0);
        assert_eq!(r.total_tokens(), 350);
    }

    #[test]
    fn over_budget_threshold_is_strict() {
        assert!(
            report(100, 200, 50, 300).is_over_budget(),
            "350 > 300 must breach"
        );
        assert!(
            !report(100, 200, 50, 350).is_over_budget(),
            "a total equal to the budget is within it"
        );
        assert!(
            !report(100, 200, 50, 8000).is_over_budget(),
            "350 < 8000 is within budget"
        );
        assert!(
            !report(100, 200, 50, 0).is_over_budget(),
            "budget 0 disables the check"
        );
    }
}
