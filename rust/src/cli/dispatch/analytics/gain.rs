//! `lean-ctx gain` — what the usage ledger says sobre saved.
//!
//! Every figure is computed from `usage.jsonl` when the command runs (see
//! [`crate::core::usage`]). Measured cost needs the proxy: without it the
//! command shows tokens saved and an estimated value, labeled as such.

use crate::cli::common::format_tokens_cli as fmt_tokens;
use crate::core::pricing::{ModelPricing, ModelQuote};
use crate::core::theme;
use crate::core::usage::{self, Period, Summary};

pub(in crate::cli::dispatch) fn cmd_gain(rest: &[String]) {
    if rest.iter().any(|a| a == "--reset") {
        if usage::reset() {
            println!("Usage ledger cleared.");
        } else {
            println!("No usage ledger to clear.");
        }
        return;
    }

    let period_arg = flag_value(rest, "--period").unwrap_or_else(|| "all".to_string());
    let Some(period) = Period::parse(&period_arg) else {
        eprintln!("Unknown period '{period_arg}'. Use: all, today, or <n>d (7d, 30d).");
        std::process::exit(2);
    };
    let limit = flag_value(rest, "--limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(10);

    crate::core::pricing::live::ensure_loaded();
    let pricing = ModelPricing::load();
    let summary = usage::summarize(&usage::load(), period, &pricing);
    let model = flag_value(rest, "--model")
        .or_else(|| summary.dominant_model().map(str::to_string))
        .or_else(detected_model);
    let quote = pricing.quote(model.as_deref());

    if rest.iter().any(|a| a == "--json") {
        println!("{}", to_json(&summary, &quote, &period_arg));
    } else if rest.iter().any(|a| a == "--by-tool") {
        print_by_tool(&summary, limit);
    } else if rest.iter().any(|a| a == "--daily") {
        print_daily(&summary);
    } else {
        print_summary(&summary, &quote, &period_arg);
    }
}

fn flag_value(rest: &[String], name: &str) -> Option<String> {
    rest.iter().enumerate().find_map(|(i, a)| {
        a.strip_prefix(&format!("{name}="))
            .map(str::to_string)
            .or_else(|| (a == name).then(|| rest.get(i + 1).cloned()).flatten())
    })
}

/// The model a host hook last reported, when the proxy saw no calls.
fn detected_model() -> Option<String> {
    let path = crate::core::data_dir::lean_ctx_data_dir()
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
        println!("{bold}Cost{rst} {dim}(measured by the proxy){rst}");
        println!("  LLM calls     {}", s.llm_calls);
        let table_priced = s.llm_calls - s.provider_priced_calls - s.local_calls;
        let mut sources = vec![
            format!("{} priced by the provider", s.provider_priced_calls),
            format!("{table_priced} from the price table"),
        ];
        if s.local_calls > 0 {
            sources.push(format!("{} local, free", s.local_calls));
        }
        println!(
            "  Real cost     ${:.2} {dim}({}){rst}",
            s.real_cost_usd,
            sources.join(", ")
        );
        println!(
            "  Without sobre ${counterfactual:.2} {dim}(real cost + saved tokens, {}){rst}",
            price_label(quote)
        );
        println!(
            "  Gain          ${saved_usd:.2} ({:.1}%) {dim}— a lower bound: removed tokens are priced once, though they would have been re-sent on later turns{rst}",
            if counterfactual > 0.0 {
                saved_usd / counterfactual * 100.0
            } else {
                0.0
            }
        );
    } else {
        println!("{bold}Value{rst} {dim}(estimated){rst}");
        println!(
            "  At least ${saved_usd:.2} {dim}({}){rst}",
            price_label(quote)
        );
        println!(
            "  {dim}No proxy in the path, so no measured cost. `lean-ctx proxy start` measures it.{rst}"
        );
    }

    let overhead = crate::core::context_overhead::ContextOverhead::measure();
    println!();
    println!("{bold}Overhead{rst}");
    println!(
        "  sobre adds {} tokens to every request {dim}({} tool definitions and the instructions){rst}",
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
            "provider_priced_calls": s.provider_priced_calls,
            "local_calls": s.local_calls,
        },
        "pricing": {
            "model": quote.model_key,
            "input_per_m": quote.cost.input_per_m,
            "estimated_price": quote.match_kind.is_estimated(),
        },
        "saved_usd": saved_usd,
        "saved_usd_kind": if measured { "lower_bound" } else { "estimated" },
        "counterfactual_cost_usd": measured.then_some(s.real_cost_usd + saved_usd),
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_value_reads_both_forms() {
        let args: Vec<String> = ["--period=7d", "--limit", "5"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(flag_value(&args, "--period").as_deref(), Some("7d"));
        assert_eq!(flag_value(&args, "--limit").as_deref(), Some("5"));
        assert_eq!(flag_value(&args, "--model"), None);
    }

    #[test]
    fn json_reports_no_measured_cost_without_calls() {
        let s = Summary::default();
        let quote = ModelPricing::load().quote(None);
        let v: serde_json::Value = serde_json::from_str(&to_json(&s, &quote, "all")).unwrap();
        assert!(v["llm"]["real_cost_usd"].is_null());
        assert_eq!(v["saved_usd_kind"], "estimated");
    }
}
