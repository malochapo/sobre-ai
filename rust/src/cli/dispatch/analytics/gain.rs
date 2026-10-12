//! `sobre gain` — what the usage ledger says sobre compressed.
//!
//! Every figure is computed from `usage.jsonl` when the command runs (see
//! [`crate::core::usage`]). It reports what sobre removed from the outputs it
//! compressed and, with the proxy, the cost the provider's token counts give.
//! It prices no saving: what an agent would have spent without sobre is not
//! something the ledger can know.

use crate::cli::args::GainArgs;
use crate::cli::common::format_tokens_cli as fmt_tokens;
use crate::core::pricing::ModelPricing;
use crate::core::theme;
use crate::core::usage::{self, Summary};

pub(in crate::cli::dispatch) fn cmd_gain(args: &GainArgs) {
    if args.reset {
        if usage::reset() {
            println!("Usage ledger cleared.");
        } else {
            println!("No usage ledger to clear.");
        }
        return;
    }

    let period_arg = args.period.label.as_str();
    let summary = usage::summarize(&usage::load(), args.period.period, &ModelPricing::load());

    if args.json {
        println!("{}", to_json(&summary, period_arg));
    } else if args.by_tool {
        print_by_tool(&summary, args.limit);
    } else if args.daily {
        print_daily(&summary);
    } else {
        print_summary(&summary, period_arg);
    }
}

/// A signed token count: it goes negative when output outgrew its input.
fn fmt_signed(tokens: i64) -> String {
    let abs = fmt_tokens(tokens.unsigned_abs());
    if tokens < 0 { format!("-{abs}") } else { abs }
}

#[allow(clippy::cast_precision_loss)]
fn pct(part: i64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64 * 100.0
    }
}

fn print_summary(s: &Summary, period: &str) {
    let (bold, dim, rst) = (theme::bold(), theme::dim(), theme::rst());
    println!("\n{bold}sobre gain{rst} {dim}({period}){rst}\n");

    if s.compression.events == 0 && s.llm_calls == 0 {
        println!("  Nothing recorded yet.\n");
        return;
    }

    let c = &s.compression;
    println!("{bold}Compression{rst}");
    println!(
        "  Removed       {} of {} tokens ({:.0}%) in {} compressions",
        fmt_signed(c.saved()),
        fmt_tokens(c.original),
        pct(c.saved(), c.original),
        c.events
    );
    if s.bounce_tokens > 0 {
        println!(
            "  Re-sent       {} tokens {dim}(full re-reads after a compressed read){rst}",
            fmt_tokens(s.bounce_tokens)
        );
    }
    println!(
        "  Net removed   {} tokens",
        fmt_signed(s.net_removed_tokens())
    );
    let by_path: Vec<String> = s
        .by_surface
        .iter()
        .map(|(surface, t)| format!("{surface} {}", fmt_signed(t.saved())))
        .collect();
    println!("  By path       {}", by_path.join(" · "));

    println!();
    if s.llm_calls > 0 {
        println!("{bold}Cost{rst} {dim}(tokens measured by the proxy, priced from the table){rst}");
        println!("  LLM calls     {}", s.llm_calls);
        let table_priced = s.llm_calls - s.local_calls;
        let mut sources = vec![format!("{table_priced} from the price table")];
        if s.local_calls > 0 {
            sources.push(format!("{} local, free", s.local_calls));
        }
        println!(
            "  Cost          ${:.2} {dim}({}){rst}",
            s.real_cost_usd,
            sources.join(", ")
        );
    } else {
        println!(
            "{bold}Cost{rst}  {dim}not measured: no proxy in the path (`sobre proxy start` measures it){rst}"
        );
    }

    let overhead = crate::core::context_overhead::ContextOverhead::measure();
    println!();
    println!("{bold}Overhead{rst}");
    println!(
        "  With its MCP server connected, sobre adds up to {} tokens to every request {dim}({} tool definitions and the instructions){rst}",
        fmt_tokens(overhead.total_tokens() as u64),
        overhead.tool_count
    );
    println!();
}

fn print_by_tool(s: &Summary, limit: usize) {
    let (bold, rst) = (theme::bold(), theme::rst());
    let mut rows: Vec<_> = s.by_tool.iter().collect();
    rows.sort_by_key(|(_, t)| std::cmp::Reverse(t.saved()));
    println!(
        "\n{bold}{:<24} {:>8} {:>10} {:>10} {:>10} {:>6}{rst}",
        "tool", "calls", "original", "sent", "removed", "%"
    );
    for (tool, t) in rows.into_iter().take(limit) {
        println!(
            "{tool:<24} {:>8} {:>10} {:>10} {:>10} {:>5.0}%",
            t.events,
            fmt_tokens(t.original),
            fmt_tokens(t.sent),
            fmt_signed(t.saved()),
            pct(t.saved(), t.original)
        );
    }
    println!();
}

fn print_daily(s: &Summary) {
    let (bold, rst) = (theme::bold(), theme::rst());
    println!(
        "\n{bold}{:<12} {:>12} {:>12}{rst}",
        "day", "removed", "cost"
    );
    for (day, (saved, cost)) in &s.by_day {
        let cost = if s.llm_calls > 0 {
            format!("${cost:.2}")
        } else {
            "-".to_string()
        };
        println!("{day:<12} {:>12} {cost:>12}", fmt_signed(*saved));
    }
    println!();
}

fn to_json(s: &Summary, period: &str) -> String {
    let measured = s.llm_calls > 0;
    serde_json::to_string_pretty(&serde_json::json!({
        "period": period,
        "compression": {
            "events": s.compression.events,
            "original_tokens": s.compression.original,
            "sent_tokens": s.compression.sent,
            "removed_tokens": s.compression.saved(),
            "bounce_tokens": s.bounce_tokens,
            "net_removed_tokens": s.net_removed_tokens(),
            "by_surface": s.by_surface.iter().map(|(k, t)| (k.clone(), serde_json::json!({
                "events": t.events, "original_tokens": t.original, "sent_tokens": t.sent,
            }))).collect::<serde_json::Map<_, _>>(),
        },
        "llm": {
            "calls": s.llm_calls,
            "input_tokens": s.input_tokens,
            "output_tokens": s.output_tokens,
            "cache_read_tokens": s.cache_read_tokens,
            "cache_write_tokens": s.cache_write_tokens,
            "cost_usd": measured.then_some(s.real_cost_usd),
            "local_calls": s.local_calls,
        },
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_reports_no_cost_without_calls() {
        let v: serde_json::Value =
            serde_json::from_str(&to_json(&Summary::default(), "all")).unwrap();
        assert!(v["llm"]["cost_usd"].is_null());
    }

    /// Only what was measured is reported: the cost of the calls the proxy
    /// forwarded, never a price for tokens that were not sent.
    #[test]
    fn json_has_the_measured_cost_and_no_priced_saving() {
        let s = Summary {
            llm_calls: 3,
            ..Summary::default()
        };
        let v: serde_json::Value = serde_json::from_str(&to_json(&s, "all")).unwrap();
        assert!(v["llm"]["cost_usd"].is_number());
        for key in [
            "saved_usd",
            "saved_usd_kind",
            "counterfactual_cost_usd",
            "pricing",
        ] {
            assert!(v.get(key).is_none(), "{key}");
        }
    }
}
