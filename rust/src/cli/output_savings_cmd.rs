//! `lean-ctx output-savings` — report how much lean-ctx reduces *output* tokens.
//!
//! A fully local, model-based estimate ([`crate::proxy::output_savings`]),
//! printed as a band and labelled as an estimate.

use crate::proxy::output_savings::{self, Savings};

/// Entry point for `lean-ctx output-savings [--json]`.
pub(crate) fn cmd_output_savings(args: &[String]) {
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help")) {
        print_usage();
        return;
    }

    let savings = output_savings::current();

    if args.iter().any(|a| a == "--json") {
        let v = output_savings::to_json(&savings);
        println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("{}", format_human(&savings));
    }
}

/// Human-readable, terminal-friendly summary.
fn format_human(s: &Savings) -> String {
    format!(
        "lean-ctx — Output Tokens Saved\n\n  Estimated reduction  ~{:.0}%  (band {:.0}–{:.0}%, model-based)\n\n  This is an estimate from the terse prose style, not a measurement.",
        s.point_pct, s.low_pct, s.high_pct
    )
}

fn print_usage() {
    println!("Usage: lean-ctx output-savings [--json]");
    println!();
    println!("Estimate how much lean-ctx reduces LLM *output* tokens.");
    println!("  (no flags)  Human-readable summary");
    println!("  --json      Machine-readable JSON");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_output_is_labelled_an_estimate() {
        let out = format_human(&output_savings::current());
        assert!(out.contains("Estimated reduction"));
        assert!(out.contains("not a measurement"));
    }
}
