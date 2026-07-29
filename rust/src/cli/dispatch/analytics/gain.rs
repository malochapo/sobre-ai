//! `lean-ctx gain` — the savings dashboard and Wrapped share cards,
//! plus the `--opportunity` and `--raw` sub-views it absorbs.

use crate::{core, tools};

pub(in crate::cli::dispatch) fn cmd_gain(rest: &[String]) {
    // Keep the latest-version cache fresh (#563): non-blocking, 24h-TTL
    // guarded, opt-out respected. Without a CLI-side trigger the cache only
    // refreshes on MCP maintenance ticks and can freeze on old versions.
    core::version_check::check_background();
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
    } else if rest.iter().any(|a| a == "--pipeline") {
        let stats_path = crate::core::paths::state_dir()
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".lean-ctx"))
            .join("pipeline_stats.json");
        if let Ok(data) = std::fs::read_to_string(&stats_path) {
            if let Ok(stats) = serde_json::from_str::<core::pipeline::PipelineStats>(&data) {
                println!("{}", stats.format_summary());
            } else {
                println!("No pipeline stats available yet (corrupt data).");
            }
        } else {
            println!("No pipeline stats available yet. Use MCP tools to generate data.");
        }
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
        // Surface available updates where users actually look (#563) — the
        // banner functions existed but had no caller, so terminals never
        // showed update hints at all.
        if let Some(banner) = core::version_check::get_update_banner() {
            println!("\n{banner}");
        }
        print_support_hint();
        print_bridge_warning();
        print_split_hint();
        print_community_hint();
    }
}

/// `gain --cost` values savings with a *resolved* model (estimated). When the
/// proxy has recorded real provider usage, point at the dashboard's usage
/// breakdown, which reports the measured per-model bill from `proxy_usage.json`.
fn print_measured_spend_hint() {
    if !crate::proxy::usage_meter::persisted_snapshot().is_empty() {
        eprintln!(
            "\n  \x1b[2m💡 Measured provider spend available — open `sobre dashboard` for the real per-model bill.\x1b[0m"
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
    let store = core::stats::load_for_display();
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

/// When stats live in more than one auto-resolved data dir, `gain` now sums them
/// for display (#500) — but each process still *writes* to its own dir, so the
/// split persists. Nudge toward consolidation once, non-alarmingly, since the
/// headline is already correct. Suppressed when `LEAN_CTX_DATA_DIR` pins one dir.
fn print_split_hint() {
    if std::env::var_os("LEAN_CTX_DATA_DIR").is_some() {
        return;
    }
    let dirs = core::data_dir::all_data_dirs_with_stats();
    if dirs.len() >= 2 {
        eprintln!(
            "\n  \x1b[2m💡 Stats span {} data dirs (summed above). Consolidate them with `lean-ctx doctor`.\x1b[0m",
            dirs.len()
        );
    }
}

fn print_community_hint() {
    let s = core::savings_ledger::summary();
    if s.total_events == 0 {
        return;
    }

    let on_board = false;
    let published = false;
    let has_name = crate::core::config::Config::load()
        .gain
        .display_name
        .is_some();

    // State-aware nudge so the path to https://leanctx.com/metrics — and how to set a
    // display name or unpublish — is always one copy-pasteable line away. Every state
    // names both the public command and the way back out (community ask: a clear
    // "how to publish/unpublish" line in the normal `gain` output).
    let body = if on_board && has_name {
        "💡 On the public leaderboard (https://leanctx.com/metrics).\n     \
         Refresh:  lean-ctx gain --publish --leaderboard\n     \
         Multiple machines?  lean-ctx gain --link  (combine into one entry)\n     \
         Remove:  lean-ctx gain --unpublish"
            .to_string()
    } else if on_board {
        "💡 You're on the leaderboard as \"anonymous\". Claim your handle:\n     \
         lean-ctx gain --publish --leaderboard --name=\"your handle\"\n     \
         Multiple machines?  lean-ctx gain --link  (combine into one entry)"
            .to_string()
    } else if published {
        "💡 You're published privately. List on the public leaderboard at https://leanctx.com/metrics:\n     \
         lean-ctx gain --publish --leaderboard --name=\"your handle\"   ·   Remove:  lean-ctx gain --unpublish"
            .to_string()
    } else {
        "💡 Join the public leaderboard at https://leanctx.com/metrics (opt-in — shares only 4 aggregate totals, never your code):\n     \
         Publish:  lean-ctx gain --publish --leaderboard --name=\"your handle\"   ·   Remove:  lean-ctx gain --unpublish"
            .to_string()
    };
    eprintln!("\n  \x1b[2m{body}\x1b[0m");
}

/// A prominent, friendly nudge to support lean-ctx financially. The engine is
/// free and never gated — this is the single place the default `gain` view asks
/// for support, rendered right under the savings hero so the people who benefit
/// most see the most natural moment to give back.
fn print_support_hint() {
    use crate::core::theme;
    let t = theme::load_theme(&crate::core::config::Config::load().theme);
    let rst = theme::rst();
    let bold = theme::bold();
    let dim = theme::dim();
    let acc = t.accent.fg();
    let heart = t.danger.fg();
    let w = 57;
    let side = t.box_side();
    let row = |content: &str| -> String {
        let padded = theme::pad_right(content, w);
        format!("  {side}{padded}{side}")
    };

    println!();
    println!("  {}", t.box_top(w));
    println!(
        "{}",
        row(&format!(
            "  {heart}\u{2665}{rst}  {bold}Enjoying lean-ctx? Keep it free & independent.{rst}"
        ))
    );
    println!("{}", row(""));
    println!(
        "{}",
        row(&format!(
            "  {dim}Back development for the price of a coffee \u{2192}{rst}"
        ))
    );
    println!(
        "{}",
        row(&format!("  {acc}{bold}https://leanctx.com/support{rst}"))
    );
    println!("  {}", t.box_bottom(w));
}

/// `gain --opportunity` — merged discover + ghost (opportunity report).
fn cmd_opportunity() {
    use crate::core::theme;

    let t = {
        let cfg = crate::core::config::Config::load();
        theme::load_theme(&cfg.theme)
    };
    let rst = theme::rst();
    let bold = theme::bold();
    let dim = theme::dim();
    let w = 57;

    let history = crate::cli::common::load_shell_history();
    let result = crate::tools::impl_discover::analyze_history(&history, 10);

    let store = core::stats::load_for_display();
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
