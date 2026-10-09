//! Environment diagnostics for sobre installation and integration.

mod checks;
mod common;
mod fix;
mod integrations;
mod lint_context;
mod overhead;
mod workspace_scope;

#[allow(clippy::wildcard_imports)]
use checks::*;
#[allow(clippy::wildcard_imports)]
use common::*;

pub(super) const GREEN: &str = "\x1b[32m";

pub(super) const RED: &str = "\x1b[31m";

pub(super) const BOLD: &str = "\x1b[1m";

pub(super) const RST: &str = "\x1b[0m";

pub(super) const DIM: &str = "\x1b[2m";

pub(super) const WHITE: &str = "\x1b[97m";

pub(super) const YELLOW: &str = "\x1b[33m";

pub(super) struct Outcome {
    pub ok: bool,
    pub line: String,
}

/// Accumulates doctor checks so the rendered ✓/✗ list and the summary tally can
/// never diverge: every scored check is counted exactly once via
/// [`Scoreboard::check`]; only explicitly-optional advisories (LSP, "not
/// configured" notes) use [`Scoreboard::info`], which renders without counting.
/// The old hand-maintained `passed`/`effective_total` pair drifted whenever a
/// check was added without bumping the total — routing every render through the
/// board makes that class of bug structurally impossible.
#[derive(Default)]
struct Scoreboard {
    passed: u32,
    total: u32,
    /// `--json`: lines are collected for one JSON document instead of printed.
    json: bool,
    entries: Vec<serde_json::Value>,
}

impl Scoreboard {
    /// A scored health check: render it and count it (pass iff `ok`).
    fn check(&mut self, outcome: &Outcome) {
        self.total += 1;
        if outcome.ok {
            self.passed += 1;
        }
        self.render(outcome, true);
    }

    fn render(&mut self, outcome: &Outcome, scored: bool) {
        if self.json {
            self.entries.push(serde_json::json!({
                "ok": outcome.ok,
                "scored": scored,
                "check": crate::core::theme::strip_ansi(&tildify_home(&outcome.line)),
            }));
        } else {
            print_check(outcome);
        }
    }

    /// A line that is neither a check nor advice: printed in text mode only.
    fn note(&self, line: &str) {
        if !self.json {
            crate::outln!("{line}");
        }
    }

    /// An optional/advisory line: render it but never count it toward the score
    /// (LSP servers, "no providers configured", MCP bridges).
    ///
    /// Deliberately a method (not a free function) so every rendered line flows
    /// through the board and each call site has to choose `check` vs `info` — the
    /// `&self` is unused by design, which is the whole point.
    fn info(&mut self, outcome: &Outcome) {
        self.render(outcome, false);
    }
}

/// Run diagnostic checks and print colored results to stdout.
/// Renders the full diagnostics board and returns how many checks need
/// attention, so `sobre doctor` can exit non-zero when something is wrong
/// (a health gate must fail loudly, not silently exit 0).
pub fn run(json: bool) -> u32 {
    let mut board = Scoreboard {
        json,
        ..Scoreboard::default()
    };

    board.note(&format!(
        "{BOLD}{WHITE}sobre doctor{RST}  {DIM}diagnostics{RST}"
    ));
    board.note(&format!("  {}", summary_line()));
    board.note(&format!("  {}\n", debug_log_line()));

    // Binary on PATH
    let path_bin = resolve_sobre_binary();
    let also_in_path_dirs = path_in_path_env();
    let bin_ok = path_bin.is_some() || also_in_path_dirs;
    let bin_line = if let Some(p) = path_bin {
        format!("{BOLD}sobre in PATH{RST}  {WHITE}{}{RST}", p.display())
    } else if also_in_path_dirs {
        format!(
            "{BOLD}sobre in PATH{RST}  {YELLOW}found via PATH walk (not resolved by `command -v`){RST}"
        )
    } else {
        format!("{BOLD}sobre in PATH{RST}  {RED}not found{RST}")
    };
    board.check(&Outcome {
        ok: bin_ok,
        line: bin_line,
    });

    // Version from PATH binary
    let ver = if bin_ok {
        sobre_version_from_path()
    } else {
        Outcome {
            ok: false,
            line: format!("{BOLD}sobre version{RST}  {RED}skipped (binary not in PATH){RST}"),
        }
    };
    board.check(&ver);

    // Data directory (respects SOBRE_DATA_DIR)
    let lean_dir = crate::core::data_dir::sobre_data_dir().ok();
    let dir_outcome = match &lean_dir {
        Some(p) if p.is_dir() => Outcome {
            ok: true,
            line: format!(
                "{BOLD}data dir{RST}  {GREEN}exists{RST}  {DIM}{}{RST}",
                p.display()
            ),
        },
        Some(p) => Outcome {
            ok: false,
            line: format!(
                "{BOLD}data dir{RST}  {RED}missing or not a directory{RST}  {DIM}{}{RST}",
                p.display()
            ),
        },
        None => Outcome {
            ok: false,
            line: format!("{BOLD}data dir{RST}  {RED}could not resolve data directory{RST}"),
        },
    };
    board.check(&dir_outcome);

    // Usage ledger + size (missing is OK: it appears with the first
    // compression)
    let usage_path = lean_dir.as_ref().map(|d| d.join("usage.jsonl"));
    let shown = usage_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let usage_outcome = match usage_path.as_ref().and_then(|p| std::fs::metadata(p).ok()) {
        Some(m) if m.is_file() => Outcome {
            ok: true,
            line: format!(
                "{BOLD}usage.jsonl{RST}  {GREEN}exists{RST}  {WHITE}{} bytes{RST}  {DIM}{shown}{RST}",
                m.len()
            ),
        },
        Some(_) => Outcome {
            ok: false,
            line: format!("{BOLD}usage.jsonl{RST}  {RED}not a file{RST}  {DIM}{shown}{RST}"),
        },
        None => Outcome {
            ok: true,
            line: if usage_path.is_some() {
                format!(
                    "{BOLD}usage.jsonl{RST}  {YELLOW}not yet created{RST}  {DIM}(appears with the first compression) {shown}{RST}"
                )
            } else {
                format!("{BOLD}usage.jsonl{RST}  {RED}could not resolve path{RST}")
            },
        },
    };
    board.check(&usage_outcome);

    // config.toml (missing is OK). It lives in the CONFIG dir
    // ($XDG_CONFIG_HOME/sobre), resolved through Config::path() to match
    // the loader's logic.
    let config_path = crate::core::config::Config::path();
    let config_outcome = match &config_path {
        Some(p) => match std::fs::metadata(p) {
            Ok(m) if m.is_file() => Outcome {
                ok: true,
                line: format!(
                    "{BOLD}config.toml{RST}  {GREEN}exists{RST}  {DIM}{}{RST}",
                    p.display()
                ),
            },
            Ok(_) => Outcome {
                ok: false,
                line: format!(
                    "{BOLD}config.toml{RST}  {RED}exists but is not a regular file{RST}  {DIM}{}{RST}",
                    p.display()
                ),
            },
            Err(_) => Outcome {
                ok: true,
                line: format!(
                    "{BOLD}config.toml{RST}  {YELLOW}not found, using defaults{RST}  {DIM}(expected at {}){RST}",
                    p.display()
                ),
            },
        },
        None => Outcome {
            ok: false,
            line: format!("{BOLD}config.toml{RST}  {RED}could not resolve path{RST}"),
        },
    };
    board.check(&config_outcome);

    // Silent config parse error trap
    let parse_outcome = config_parse_outcome();
    board.check(&parse_outcome);

    // Search roots that never resolve
    let search_roots = search_roots_outcome();
    board.check(&search_roots);

    // Managed ONNX Runtime: present + readable on
    // embedding-enabled builds. Silent when not provisioned (opt-in).
    if let Some(managed_ort) = managed_ort_outcome() {
        board.check(&managed_ort);
    }

    // Proxy upstreams
    let proxy_outcome = proxy_upstream_outcome();
    board.check(&proxy_outcome);

    // MCP
    let mcp = mcp_config_outcome();
    board.check(&mcp);
    let user_scope_mcp_locations = dirs::home_dir()
        .map(|home| sobre_mcp_location_names(&home))
        .unwrap_or_default();

    // WSL2 + VS Code: surface the upstream first-call invoke race
    if let Some(wsl_hint) = wsl_vscode_mcp_outcome() {
        board.check(&wsl_hint);
    }

    // Workspace-scope MCP (optional; only when a project-local config exists)
    let workspace_scope = workspace_scope::workspace_scope_outcome(&user_scope_mcp_locations);
    if let Some(ref ws) = workspace_scope {
        board.check(ws);
    }

    if let Some(log_path) = crate::core::startup_guard::crash_loop_log_path(
        crate::core::startup_guard::MCP_PROCESS_NAME,
    ) && log_path.exists()
        && let Ok(contents) = std::fs::read_to_string(&log_path)
    {
        let lines: Vec<&str> = contents.lines().collect();
        if lines.len() >= 5 {
            board.info(&Outcome {
                ok: false,
                line: format!(
                    "{BOLD}Crash-loop log{RST}  {YELLOW}{} recent restarts{RST}  {DIM}({}){RST}",
                    lines.len(),
                    display_user_path(&log_path)
                ),
            });
        }
    }

    // Session state (project_root + shell_cwd)
    let session_outcome = session_state_outcome();
    board.check(&session_outcome);

    // Claude Code instruction truncation guard
    let claude_truncation = claude_truncation_outcome();
    if let Some(ref ct) = claude_truncation {
        board.check(ct);
    }

    // BM25 cache health
    let bm25_health = bm25_cache_health_outcome();
    board.check(&bm25_health);

    // Semantic index runtime status (state/timing/persistence) for the
    // active project — surfaces a stuck "warming" index.
    let semantic_index = semantic_index_outcome();
    if let Some(ref check) = semantic_index {
        board.check(check);
    }

    // Capacity warnings (memory stores near limits)
    let cap_warnings = capacity_warnings();
    for cw in &cap_warnings {
        board.check(cw);
    }

    // Orphaned knowledge stores (deleted projects — reclaimable bloat)
    let orphan_outcome = orphaned_knowledge_outcome();
    board.check(&orphan_outcome);

    // Proxy health
    let proxy_health = proxy_health_outcome();
    board.check(&proxy_health);

    // Proxy upstream drift: running proxy serves a different upstream
    // than config.toml resolves to (env override masking config). Only surfaces
    // when the proxy is up and actually drifting.
    let upstream_drift = proxy_upstream_drift_outcome();
    if let Some(ref check) = upstream_drift {
        board.check(check);
    }

    // Stale proxy env (ANTHROPIC_BASE_URL pointing to local proxy while proxy is not enabled)
    let stale_env = stale_proxy_env_outcome();
    if let Some(ref check) = stale_env {
        board.check(check);
    }

    // Claude Pro/Max subscription routed through the proxy without an API key
    let subscription_conflict = proxy_subscription_conflict_outcome();
    if let Some(ref check) = subscription_conflict {
        board.check(check);
    }

    // LSP servers (optional, informational)
    board.note(&format!(
        "\n  {BOLD}{WHITE}LSP (optional — for sobre_refactor):{RST}"
    ));
    let lsp_outcomes = lsp_server_outcomes();
    for lsp_check in &lsp_outcomes {
        board.info(lsp_check);
    }

    // Tool-schema footprint (informational, not scored). There is one surface —
    // the registry — so report its size rather than a tier name. This used to
    // branch on a pinned tier and print it; that was already a bug once, where it
    // claimed "power" right after the wizard chose lean.
    let advertised = crate::server::registry::build_registry().tool_defs().len();
    board.note(&format!(
        "  {BOLD}Tools{RST}  {WHITE}{advertised} advertised{RST}  \
         {DIM}one surface, every client{RST}"
    ));

    // The board counted exactly what it rendered — the displayed ✓/✗ list and
    // this tally can no longer drift apart.
    let passed = board.passed;
    let total = board.total;
    let needs_attention = total.saturating_sub(passed);
    if json {
        let report = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "passed": passed,
            "total": total,
            "tools_advertised": advertised,
            "checks": board.entries,
        });
        crate::outln!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
        );
        return needs_attention;
    }
    crate::outln!();
    crate::outln!(
        "  {BOLD}{WHITE}Summary:{RST}  {GREEN}{passed}{RST}{DIM}/{total}{RST} checks passed"
    );
    if needs_attention > 0 {
        crate::outln!(
            "  {YELLOW}{needs_attention} check(s) need attention.{RST}  Auto-repair what's fixable:  {BOLD}sobre doctor --fix{RST}"
        );
    } else {
        crate::outln!("  {GREEN}Everything looks good.{RST}");
    }
    crate::outln!("  {DIM}LSP servers are optional enhancements (not counted in score){RST}");
    crate::outln!("  {DIM}sobre {}{RST}", env!("CARGO_PKG_VERSION"));

    needs_attention
}

/// One line on the install: the running version and which detected agents
/// have a sobre MCP entry.
fn summary_line() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let detected: Vec<_> = dirs::home_dir()
        .map(|home| crate::core::editor_registry::build_targets(&home))
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.detect_path.exists())
        .collect();
    let connected: Vec<&str> = detected
        .iter()
        .filter(|t| std::fs::read_to_string(&t.config_path).is_ok_and(|s| s.contains("sobre")))
        .map(|t| t.name)
        .collect();
    let names = if connected.is_empty() {
        String::new()
    } else {
        format!(" ({})", connected.join(", "))
    };
    format!(
        "{WHITE}sobre {version}{RST}  agents connected: {}/{} detected{names}",
        connected.len(),
        detected.len()
    )
}

/// Where the opt-in debug log (config key `debug_log`) is written.
fn debug_log_line() -> String {
    let state = if crate::core::debug_log::is_enabled() {
        "on"
    } else {
        "off, set debug_log = true to record"
    };
    match crate::core::debug_log::log_path() {
        Some(p) => format!("{DIM}debug log: {} ({state}){RST}", p.display()),
        None => format!("{DIM}debug log: state dir unresolvable ({state}){RST}"),
    }
}

pub fn run_cli(args: &[String]) -> i32 {
    let (sub, rest) = match args.first().map(String::as_str) {
        Some("integrations") => ("integrations", &args[1..]),
        Some("overhead") => ("overhead", &args[1..]),
        Some("lint-context") => ("lint-context", &args[1..]),
        _ => ("", args),
    };

    crate::cli::spec::check_or_exit(
        &if sub.is_empty() {
            "doctor".to_string()
        } else {
            format!("doctor {sub}")
        },
        rest,
    );
    let fix = rest.iter().any(|a| a == "--fix");
    let json = rest.iter().any(|a| a == "--json");
    let gate = rest.iter().any(|a| a == "--gate");

    if sub == "overhead" {
        return overhead::run_overhead(json, gate);
    }

    if sub == "lint-context" {
        return lint_context::run_lint_context(json);
    }

    if sub == "integrations" {
        if fix {
            let _ = fix::run_fix(&fix::DoctorFixOptions { json: false });
        }
        return integrations::run_integrations(&integrations::IntegrationsOptions { json });
    }

    if !fix {
        // Non-zero exit when checks need attention so `sobre doctor` works
        // as a CI/health gate, not just a pretty printer.
        return i32::from(run(json) > 0);
    }

    match fix::run_fix(&fix::DoctorFixOptions { json }) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("doctor --fix failed: {e}");
            1
        }
    }
}

pub fn compact_score() -> (u32, u32) {
    let mut passed = 0u32;
    let total = 4u32;

    if resolve_sobre_binary().is_some() || path_in_path_env() {
        passed += 1;
    }
    let lean_dir = crate::core::data_dir::sobre_data_dir().ok();
    if lean_dir.as_ref().is_some_and(|p| p.is_dir()) {
        passed += 1;
    }
    // Same rule as `sobre doctor`: a missing usage ledger is fine (it appears
    // with the first compression); only something that is not a file fails.
    if lean_dir
        .as_ref()
        .map(|d| d.join("usage.jsonl"))
        .is_some_and(|p| std::fs::metadata(p).map_or(true, |m| m.is_file()))
    {
        passed += 1;
    }
    if mcp_config_outcome().ok {
        passed += 1;
    }

    (passed, total)
}

pub(super) fn print_compact_status(passed: u32, total: u32) {
    let status = if passed == total {
        format!("{GREEN}✓ {total} quick checks passed{RST} {DIM}(sobre doctor runs them all){RST}")
    } else {
        format!("{YELLOW}{passed}/{total} passed{RST} — run {BOLD}sobre doctor{RST} for details")
    };
    crate::outln!("  {status}");
}

#[cfg(test)]
mod tests {

    // `checks::capacity_level`, the classification `capacity_warnings` applies
    // to each store: at or below its cap is at most a WARN (healthy, eviction
    // keeps it there); only a store *over* cap is CRIT.
    #[test]
    fn capacity_below_80_no_warning() {
        assert!(super::checks::capacity_level("facts", 100, 200).is_none());
        assert!(super::checks::capacity_level("facts", 159, 200).is_none());
    }

    #[test]
    fn capacity_at_80_yellow_warning() {
        let result = super::checks::capacity_level("facts", 160, 200);
        assert!(result.is_some());
        let (critical, msg) = result.unwrap();
        assert!(!critical);
        assert!(msg.contains("160/200"));
        assert!(msg.contains("80%"));
    }

    #[test]
    fn capacity_at_92_yellow_warning() {
        let result = super::checks::capacity_level("facts", 185, 200);
        assert!(result.is_some());
        let (critical, msg) = result.unwrap();
        assert!(!critical);
        assert!(msg.contains("185/200"));
        assert!(msg.contains("92%"));
    }

    #[test]
    fn capacity_at_95_is_warning_not_critical() {
        let result = super::checks::capacity_level("facts", 190, 200);
        assert!(result.is_some());
        let (critical, msg) = result.unwrap();
        assert!(!critical, "95% is full-but-healthy, not over cap");
        assert!(msg.contains("190/200"));
        assert!(msg.contains("95%"));
    }

    #[test]
    fn capacity_at_100_is_warning_not_critical() {
        // A store exactly at its cap is healthy — eviction keeps it there.
        let result = super::checks::capacity_level("facts", 200, 200);
        assert!(result.is_some());
        let (critical, _) = result.unwrap();
        assert!(!critical);
    }

    #[test]
    fn capacity_over_100_is_critical() {
        // Genuinely over cap => eviction is not keeping up (regression guard for
        // the 206/200 "CRIT" that fired before lifecycle eviction was fixed).
        let result = super::checks::capacity_level("facts", 206, 200);
        assert!(result.is_some());
        let (critical, msg) = result.unwrap();
        assert!(critical);
        assert!(msg.contains("206/200"));
        assert!(msg.contains("103%"));
    }

    #[test]
    fn capacity_zero_limit_skipped() {
        assert!(super::checks::capacity_level("facts", 50, 0).is_none());
    }
}
