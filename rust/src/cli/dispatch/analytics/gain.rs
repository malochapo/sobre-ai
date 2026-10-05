//! `lean-ctx gain` — the savings dashboard and Wrapped share cards,
//! plus the `--opportunity` and `--raw` sub-views it absorbs.

use crate::{core, tools};

pub(in crate::cli::dispatch) fn cmd_gain(rest: &[String]) {
    if rest.iter().any(|a| a == "--reset") {
        core::stats::reset_all();
        println!("Stats reset. All token savings data cleared.");
        return;
    }
    if rest.iter().any(|a| a == "--live") {
        core::stats::gain_live();
        return;
    }
    let model = rest.iter().enumerate().find_map(|(i, a)| {
        if let Some(v) = a.strip_prefix("--model=") {
            return Some(v.to_string());
        }
        if a == "--model" {
            return rest.get(i + 1).cloned();
        }
        None
    });
    let period = rest
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            if let Some(v) = a.strip_prefix("--period=") {
                return Some(v.to_string());
            }
            if a == "--period" {
                return rest.get(i + 1).cloned();
            }
            None
        })
        .unwrap_or_else(|| "all".to_string());
    let limit = rest
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            if let Some(v) = a.strip_prefix("--limit=") {
                return v.parse::<usize>().ok();
            }
            if a == "--limit" {
                return rest.get(i + 1).and_then(|v| v.parse::<usize>().ok());
            }
            None
        })
        .unwrap_or(10);

    let _open = rest.iter().any(|a| a == "--open");

    if rest.iter().any(|a| a == "--no-cache-adjust") {
        core::context_overhead::set_no_cache_adjust(true);
    }

    if rest.iter().any(|a| a == "--by-tool") {
        cmd_by_tool(rest, limit);
    } else if rest.iter().any(|a| a == "--graph") {
        println!("{}", core::stats::format_gain_graph());
    } else if rest.iter().any(|a| a == "--daily") {
        println!("{}", core::stats::format_gain_daily());
    // Before `--json`: `--measured` selects a different data source, and it
    // renders in either shape. Tested after `--json` it would be unreachable
    // whenever both flags are passed.
    } else if rest.iter().any(|a| a == "--measured") {
        print_measured_spend(rest.iter().any(|a| a == "--json"));
    } else if rest.iter().any(|a| a == "--json") {
        println!(
            "{}",
            tools::impl_gain::handle("json", Some(&period), model.as_deref(), Some(limit))
        );
    } else if rest.iter().any(|a| a == "--score") {
        println!(
            "{}",
            tools::impl_gain::handle("score", None, model.as_deref(), Some(limit))
        );
    } else if rest.iter().any(|a| a == "--cost") {
        println!(
            "{}",
            tools::impl_gain::handle("cost", None, model.as_deref(), Some(limit))
        );
        print_measured_spend_hint();
    } else if rest.iter().any(|a| a == "--tasks") {
        println!(
            "{}",
            tools::impl_gain::handle("tasks", None, None, Some(limit))
        );
    } else if rest.iter().any(|a| a == "--agents") {
        println!(
            "{}",
            tools::impl_gain::handle("agents", None, None, Some(limit))
        );
    } else if rest.iter().any(|a| a == "--heatmap") {
        println!(
            "{}",
            tools::impl_gain::handle("heatmap", None, None, Some(limit))
        );
    } else if rest.iter().any(|a| a == "--wrapped") {
        println!(
            "{}",
            tools::impl_gain::handle("wrapped", Some(&period), model.as_deref(), Some(limit))
        );
    } else if rest.iter().any(|a| a == "--deep") {
        // Body first, then the extra themed sections, then the footer — so the
        // tips / Context OS panel land at the very end instead of mid-dashboard.
        println!("{}", core::stats::format_gain_body());
        print!(
            "{}",
            tools::impl_gain::format_deep_themed(model.as_deref(), limit)
        );
        println!("{}", core::stats::format_gain_footer());
    } else if rest.iter().any(|a| a == "--opportunity") {
        cmd_opportunity();
    } else if rest.iter().any(|a| a == "--raw") {
        cmd_stats_raw(rest);
    } else {
        println!("{}", core::stats::format_gain_hero());
        print_bridge_warning();
    }
}

/// `gain --measured` — the real per-model bill the proxy recorded, as opposed
/// to `--cost`, which prices our own token counts from a rate table. The
/// dashboard has always shown this; without a CLI view the data was collected
/// and unreadable.
fn print_measured_spend(json: bool) {
    let spend = crate::proxy::usage_meter::persisted_snapshot();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&spend).unwrap_or_else(|_| "[]".into())
        );
        return;
    }
    if spend.is_empty() {
        println!("No measured provider spend recorded yet.");
        println!(
            "Route traffic through `sobre proxy` to collect it, or use `gain --cost` for the estimate."
        );
        return;
    }
    println!("Measured provider spend (from the proxy's recorded usage)\n");
    println!(
        "{:<34} {:>6} {:>12} {:>12} {:>10}",
        "MODEL", "REQS", "INPUT", "OUTPUT", "COST USD"
    );
    let mut total = 0.0;
    for m in &spend {
        total += m.cost_usd;
        // Flag the rows we could not price from a provider-reported figure, so
        // an estimate is never silently read as a bill.
        let mark = if m.measured_requests < m.requests {
            " ~"
        } else {
            ""
        };
        println!(
            "{:<34} {:>6} {:>12} {:>12} {:>10.4}{}",
            m.model, m.requests, m.input_tokens, m.output_tokens, m.cost_usd, mark
        );
    }
    println!("\n{:<34} {:>43.4}", "TOTAL", total);
    if spend.iter().any(|m| m.measured_requests < m.requests) {
        println!("\n  ~ partly estimated: the provider did not report a cost for every request.");
    }
}

/// `gain --cost` values savings with a *resolved* model (estimated). When the
/// proxy has recorded real provider usage, point at the measured view so the
/// estimate is never mistaken for the bill.
fn print_measured_spend_hint() {
    if !crate::proxy::usage_meter::persisted_snapshot().is_empty() {
        eprintln!(
            "\n  \x1b[2m💡 Measured provider spend available — `sobre gain --measured` for the real per-model bill.\x1b[0m"
        );
    }
}

/// When the bridge is not engaged, the proxy is no longer feeding live request
/// stats. Surface that caveat so `gain` never implies savings it cannot measure
/// (GitHub #361/#271) — **but only when there are genuinely no savings to show**.
///
/// `gain` also measures MCP-tool savings directly (reads/search/shell via the
/// savings ledger + stats), which need no proxy at all. Emitting a "proxy down —
/// savings cannot be measured" line above a real, MCP-measured headline wrongly
/// told MCP-only users their numbers were untrustworthy (#500). Gate on the
/// displayed total so the warning fires only for a true zero.
fn print_bridge_warning() {
    use crate::core::gain::bridge_status::{BridgeEngagement, BridgeStatus};
    let store = core::stats::load();
    let saved = store
        .total_input_tokens
        .saturating_sub(store.total_output_tokens);
    if saved > 0 {
        return;
    }
    let bridge = BridgeStatus::detect();
    if bridge.engagement != BridgeEngagement::Engaged {
        eprintln!("\n  \x1b[33m⚠ {}\x1b[0m", bridge.summary_line());
    }
}

/// `gain --opportunity` — merged discover + ghost (opportunity report).
fn cmd_opportunity() {
    use crate::core::theme;

    let t = { crate::core::theme::Theme::default() };
    let rst = theme::rst();
    let bold = theme::bold();
    let dim = theme::dim();
    let w = 57;

    let history = crate::cli::common::load_shell_history();
    let result = crate::tools::impl_discover::analyze_history(&history, 10);

    let store = core::stats::load();
    let optimized_cmds = store.total_commands;

    if result.missed_commands.is_empty() {
        println!(
            "\n  {sc}{bold}All compressible commands are already using lean-ctx!{rst}\n  {dim}{optimized_cmds} commands optimized.{rst}\n",
            sc = t.success.fg(),
        );
        return;
    }

    let total_missed: u64 = result
        .missed_commands
        .iter()
        .map(|c| c.estimated_tokens as u64)
        .sum();
    let total_count: u64 = result.missed_commands.iter().map(|c| c.count as u64).sum();

    println!();
    println!("  {}", t.box_top(w));
    let side = t.box_side();
    let padded = |s: &str| -> String {
        let p = theme::pad_right(s, w);
        format!("  {side}{p}{side}")
    };
    println!(
        "{}",
        padded(&format!("  {bold}{}Opportunity Report{rst}", t.accent.fg()))
    );
    println!("{}", padded(""));
    println!(
        "{}",
        padded(&format!(
            "  {bold}~{}{rst} {dim}tokens/month going uncompressed{rst}",
            crate::core::wrapped::format_tokens(total_missed),
        ))
    );
    println!("{}", padded(""));

    for entry in result.missed_commands.iter().take(8) {
        let line = format!(
            "  {dim}{:<12}{rst} {:>4}x   {bold}~{}{rst} {dim}tokens recoverable{rst}",
            entry.prefix,
            entry.count,
            crate::core::wrapped::format_tokens(entry.estimated_tokens as u64),
        );
        println!("{}", padded(&line));
    }

    println!("{}", padded(""));
    let opt_line = format!(
        "  {dim}Already optimized: {optimized_cmds} commands ({pct}%){rst}",
        pct = total_count
            .checked_add(optimized_cmds)
            .and_then(|total| optimized_cmds.checked_mul(100)?.checked_div(total))
            .unwrap_or(100)
    );
    println!("{}", padded(&opt_line));
    println!("  {}", t.box_bottom(w));
    println!();
    let sec = t.secondary.fg();
    println!("  {sec}lean-ctx init --global{rst}   {dim}Compress everything{rst}");
    println!();
}

/// `gain --by-tool` — per-tool savings breakdown from the verified savings
/// ledger (calls, tokens saved, USD, compression %). The aggregate `gain`
/// headline hides which tool earned the savings; this surfaces it. Honors
/// `--limit` and `--json`.
fn cmd_by_tool(rest: &[String], limit: usize) {
    use std::collections::HashMap;

    #[derive(Default)]
    struct Agg {
        calls: u64,
        saved_tokens: u64,
        saved_usd: f64,
        baseline_tokens: u64,
    }

    let mut by_tool: HashMap<String, Agg> = HashMap::new();
    for e in core::savings_ledger::all_events() {
        let a = by_tool.entry(e.tool).or_default();
        a.calls += 1;
        a.saved_tokens += e.saved_tokens;
        a.saved_usd += e.saved_usd;
        a.baseline_tokens += e.baseline_tokens;
    }

    let mut rows: Vec<(String, Agg)> = by_tool.into_iter().collect();
    // Heaviest saver first; tie-break by name so the view is deterministic.
    rows.sort_by(|a, b| {
        b.1.saved_tokens
            .cmp(&a.1.saved_tokens)
            .then_with(|| a.0.cmp(&b.0))
    });
    rows.truncate(limit);

    let pct = |a: &Agg| -> f64 {
        if a.baseline_tokens > 0 {
            a.saved_tokens as f64 / a.baseline_tokens as f64 * 100.0
        } else {
            0.0
        }
    };

    if rest.iter().any(|a| a == "--json") {
        let arr: Vec<_> = rows
            .iter()
            .map(|(tool, a)| {
                serde_json::json!({
                    "tool": tool,
                    "calls": a.calls,
                    "saved_tokens": a.saved_tokens,
                    "saved_usd": a.saved_usd,
                    "baseline_tokens": a.baseline_tokens,
                    "saved_pct": pct(a),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
        );
        return;
    }

    if rows.is_empty() {
        println!(
            "No per-tool savings recorded yet. Use lean-ctx-backed tools (ctx_read, \
             ctx_search, ctx_shell) to populate the ledger."
        );
        return;
    }

    println!("Savings by tool (most saved first):\n");
    println!(
        "  {:<16} {:>8} {:>12} {:>10} {:>7}",
        "TOOL", "CALLS", "SAVED", "USD", "SAVED%"
    );
    for (tool, a) in &rows {
        println!(
            "  {:<16} {:>8} {:>12} {:>10} {:>6.1}%",
            tool,
            a.calls,
            core::wrapped::format_tokens(a.saved_tokens),
            format!("${:.2}", a.saved_usd),
            pct(a),
        );
    }
}

/// `gain --raw` — plain stats (absorbs former `lean-ctx stats` command).
fn cmd_stats_raw(rest: &[String]) {
    if rest.iter().any(|a| a == "--json" || a == "json") {
        let store = core::stats::load();
        println!(
            "{}",
            serde_json::to_string_pretty(&store).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        let store = core::stats::load();
        let input_saved = store
            .total_input_tokens
            .saturating_sub(store.total_output_tokens);
        let pct = if store.total_input_tokens > 0 {
            input_saved as f64 / store.total_input_tokens as f64 * 100.0
        } else {
            0.0
        };
        println!("Commands:    {}", store.total_commands);
        println!("Input:       {} tokens", store.total_input_tokens);
        println!("Output:      {} tokens", store.total_output_tokens);
        println!("Saved:       {input_saved} tokens ({pct:.1}%)");
        println!();
        println!("CEP sessions:  {}", store.cep.sessions);
        println!(
            "CEP tokens:    {} → {}",
            store.cep.total_tokens_original, store.cep.total_tokens_compressed
        );
    }
}
