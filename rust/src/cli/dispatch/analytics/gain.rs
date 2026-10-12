//! `sobre gain` — what the usage ledger says sobre saved.
//!
//! Every figure is computed from `usage.jsonl` when the command runs (see
//! [`crate::core::usage`]). Measured cost needs the proxy: without it the
//! command shows tokens saved and an estimated value, labeled as such.

use crate::cli::args::GainArgs;
use crate::cli::common::format_tokens_cli as fmt_tokens;
use crate::core::pricing::{ModelPricing, ModelQuote};
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
    let pricing = ModelPricing::load();
    let summary = usage::summarize(&usage::load(), args.period.period, &pricing);
    let asked = args.model.clone();
    let model = asked
        .clone()
        .or_else(|| summary.dominant_model().map(str::to_string))
        .or_else(detected_model);
    let quote = pricing.quote(model.as_deref());
    // A model the user named but that has no price is a mistake to report,
    // not a reason to price at the blended fallback.
    if let Some(name) = asked
        && quote.match_kind == crate::core::pricing::PricingMatchKind::Fallback
    {
        eprintln!(
            "Unknown model '{name}'. Priced models: {}",
            pricing.known_models().join(", ")
        );
        std::process::exit(2);
    }

    if args.json {
        println!("{}", to_json(&summary, &quote, period_arg));
    } else if args.by_tool {
        print_by_tool(&summary, args.limit);
    } else if args.daily {
        print_daily(&summary);
    } else {
        print_summary(&summary, &quote, period_arg);
    }
}

/// The model a host hook last reported, when the proxy saw no calls.
fn detected_model() -> Option<String> {
    let path = crate::core::data_dir::sobre_data_dir()
        .ok()?
        .join("detected_model.json");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v.get("model")?.as_str().map(str::to_string)
}

/// A signed token count: savings go negative when output outgrew its input.
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

fn price_label(quote: &ModelQuote) -> String {
    let estimated = if quote.match_kind.is_estimated() {
        ", estimated price"
    } else {
        ""
    };
    format!(
        "{} at ${:.2}/M input{estimated}",
        quote.model_key, quote.cost.input_per_m
    )
}

fn print_summary(s: &Summary, quote: &ModelQuote, period: &str) {
    let (bold, dim, rst) = (theme::bold(), theme::dim(), theme::rst());
    println!("\n{bold}sobre gain{rst} {dim}({period}){rst}\n");

    if s.compression.events == 0 && s.llm_calls == 0 {
        println!("  Nothing recorded yet. Savings appear here once an agent uses sobre.\n");
        return;
    }

    let c = &s.compression;
    println!("{bold}Compression{rst}");
    println!(
        "  Saved         {} of {} tokens ({:.0}%) in {} compressions",
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
        "  Net saved     {} tokens",
        fmt_signed(s.net_saved_tokens())
    );
    let by_path: Vec<String> = s
        .by_surface
        .iter()
        .map(|(surface, t)| format!("{surface} {}", fmt_signed(t.saved())))
        .collect();
    println!("  By path       {}", by_path.join(" · "));

    let saved_usd = s.saved_usd(quote);
    println!();
    if s.llm_calls > 0 {
        let counterfactual = s.real_cost_usd + saved_usd;
        println!("{bold}Cost{rst} {dim}(tokens measured by the proxy, priced from the table){rst}");
        println!("  LLM calls     {}", s.llm_calls);
        let table_priced = s.llm_calls - s.local_calls;
        let mut sources = vec![format!("{table_priced} from the price table")];
        if s.local_calls > 0 {
            sources.push(format!("{} local, free", s.local_calls));
        }
        println!(
            "  Real cost     ${:.2} {dim}({}){rst}",
            s.real_cost_usd,
            sources.join(", ")
        );
        if s.local_calls == s.llm_calls {
            // Tokens a local model never read cost nothing: the saving is
            // context and time, not money.
            println!("  Gain          {dim}none in money: every call went to a local model{rst}");
        } else {
            println!(
                "  Without sobre ${counterfactual:.2} {dim}(real cost + saved tokens, {}){rst}",
                price_label(quote)
            );
            println!(
                "  Gain          ${saved_usd:.2} ({:.1}%) {dim}— an estimate: saved tokens are priced as uncached input; prompt caching would have made many of them cheaper, and the proxy counts history it re-compresses on each request{rst}",
                if counterfactual > 0.0 {
                    saved_usd / counterfactual * 100.0
                } else {
                    0.0
                }
            );
            if s.local_calls > 0 {
                println!(
                    "                {dim}saved tokens of the local calls are priced as hosted ones{rst}"
                );
            }
        }
    } else {
        println!("{bold}Value{rst} {dim}(estimated){rst}");
        println!(
            "  About ${saved_usd:.2} {dim}({}; saved tokens priced as uncached input){rst}",
            price_label(quote)
        );
        println!(
            "  {dim}No proxy in the path, so no measured cost. `sobre proxy start` measures it.{rst}"
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
        "tool", "calls", "original", "sent", "saved", "%"
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
        "day", "saved", "real cost"
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

fn to_json(s: &Summary, quote: &ModelQuote, period: &str) -> String {
    let saved_usd = s.saved_usd(quote);
    let measured = s.llm_calls > 0;
    serde_json::to_string_pretty(&serde_json::json!({
        "period": period,
        "compression": {
            "events": s.compression.events,
            "original_tokens": s.compression.original,
            "sent_tokens": s.compression.sent,
            "removed_tokens": s.compression.saved(),
            "bounce_tokens": s.bounce_tokens,
            "net_saved_tokens": s.net_saved_tokens(),
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
            "real_cost_usd": measured.then_some(s.real_cost_usd),
            "local_calls": s.local_calls,
        },
        "pricing": {
            "model": quote.model_key,
            "input_per_m": quote.cost.input_per_m,
            "estimated_price": quote.match_kind.is_estimated(),
        },
        "saved_usd": saved_usd,
        // Measured or not, the saving is priced against tokens that were
        // never sent: only `real_cost_usd` is measured.
        "saved_usd_kind": "estimated",
        "counterfactual_cost_usd": measured.then_some(s.real_cost_usd + saved_usd),
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_reports_no_measured_cost_without_calls() {
        let s = Summary::default();
        let quote = ModelPricing::load().quote(None);
        let v: serde_json::Value = serde_json::from_str(&to_json(&s, &quote, "all")).unwrap();
        assert!(v["llm"]["real_cost_usd"].is_null());
        assert_eq!(v["saved_usd_kind"], "estimated");
    }

    /// Calls measured through the proxy measure the cost, not the saving.
    #[test]
    fn json_calls_the_saving_an_estimate_with_measured_calls() {
        let s = Summary {
            llm_calls: 3,
            ..Summary::default()
        };
        let quote = ModelPricing::load().quote(None);
        let v: serde_json::Value = serde_json::from_str(&to_json(&s, &quote, "all")).unwrap();
        assert!(v["llm"]["real_cost_usd"].is_number());
        assert_eq!(v["saved_usd_kind"], "estimated");
    }
}
