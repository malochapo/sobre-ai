//! Environment diagnostics for lean-ctx installation and integration.

mod checks;
mod common;
mod deprecations;
mod fix;
mod integrations;
mod lint_context;
mod overhead;
mod report;
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
/// never diverge (#433): every scored check is counted exactly once via
/// [`Scoreboard::check`]; only explicitly-optional advisories (LSP, "not
/// configured" notes) use [`Scoreboard::info`], which renders without counting.
/// The old hand-maintained `passed`/`effective_total` pair drifted whenever a
/// check was added without bumping the total — routing every render through the
/// board makes that class of bug structurally impossible.
#[derive(Default)]
struct Scoreboard {
    passed: u32,
    total: u32,
}

impl Scoreboard {
    /// A scored health check: render it and count it (pass iff `ok`).
    fn check(&mut self, outcome: &Outcome) {
        self.total += 1;
        if outcome.ok {
            self.passed += 1;
        }
        print_check(outcome);
    }

    /// An optional/advisory line: render it but never count it toward the score
    /// (LSP servers, "no providers configured", MCP bridges, plan-mode presence).
    ///
    /// Deliberately a method (not a free function) so every rendered line flows
    /// through the board and each call site has to choose `check` vs `info` — the
    /// `&self` is unused by design, which is the whole point.
    #[allow(clippy::unused_self)]
    fn info(&self, outcome: &Outcome) {
        print_check(outcome);
    }
}

/// Run diagnostic checks and print colored results to stdout.
/// Renders the full diagnostics board and returns how many checks need
/// attention, so `lean-ctx doctor` can exit non-zero when something is wrong
/// (a health gate must fail loudly, not silently exit 0).
pub fn run() -> u32 {
    let mut board = Scoreboard::default();

    println!("{BOLD}{WHITE}lean-ctx doctor{RST}  {DIM}diagnostics{RST}\n");

    // 1) Binary on PATH
    let path_bin = resolve_lean_ctx_binary();
    let also_in_path_dirs = path_in_path_env();
    let bin_ok = path_bin.is_some() || also_in_path_dirs;
    let bin_line = if let Some(p) = path_bin {
        format!("{BOLD}lean-ctx in PATH{RST}  {WHITE}{}{RST}", p.display())
    } else if also_in_path_dirs {
        format!(
            "{BOLD}lean-ctx in PATH{RST}  {YELLOW}found via PATH walk (not resolved by `command -v`){RST}"
        )
    } else {
        format!("{BOLD}lean-ctx in PATH{RST}  {RED}not found{RST}")
    };
    board.check(&Outcome {
        ok: bin_ok,
        line: bin_line,
    });

    // 2) Version from PATH binary
    let ver = if bin_ok {
        lean_ctx_version_from_path()
    } else {
        Outcome {
            ok: false,
            line: format!("{BOLD}lean-ctx version{RST}  {RED}skipped (binary not in PATH){RST}"),
        }
    };
    board.check(&ver);

    // 3) data directory (respects LEAN_CTX_DATA_DIR)
    let lean_dir = crate::core::data_dir::lean_ctx_data_dir().ok();
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

    // 4) stats.json + size
    let stats_path = lean_dir.as_ref().map(|d| d.join("stats.json"));
    let stats_outcome = match stats_path.as_ref().and_then(|p| std::fs::metadata(p).ok()) {
        Some(m) if m.is_file() => {
            let size = m.len();
            let path_display = if let Some(p) = stats_path.as_ref() {
                p.display().to_string()
            } else {
                String::new()
            };
            Outcome {
                ok: true,
                line: format!(
                    "{BOLD}stats.json{RST}  {GREEN}exists{RST}  {WHITE}{size} bytes{RST}  {DIM}{path_display}{RST}",
                ),
            }
        }
        Some(_m) => {
            let path_display = if let Some(p) = stats_path.as_ref() {
                p.display().to_string()
            } else {
                String::new()
            };
            Outcome {
                ok: false,
                line: format!(
                    "{BOLD}stats.json{RST}  {RED}not a file{RST}  {DIM}{path_display}{RST}",
                ),
            }
        }
        None => Outcome {
            ok: true,
            line: match &stats_path {
                Some(p) => format!(
                    "{BOLD}stats.json{RST}  {YELLOW}not yet created{RST}  {DIM}(will appear after first use) {}{RST}",
                    p.display()
                ),
                None => format!("{BOLD}stats.json{RST}  {RED}could not resolve path{RST}"),
            },
        },
    };
    board.check(&stats_outcome);

    // 5) config.toml (missing is OK). It lives in the CONFIG dir
    // ($XDG_CONFIG_HOME/lean-ctx), resolved through Config::path() to match
    // the loader's logic (#435).
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

    // 5b) Shell gate (effective runtime view + silent-parse-error trap, #341)
    let gate_outcome = shell_gate_outcome();
    board.check(&gate_outcome);

    // 5b2) Path jail (effective state + dead allow_paths entries, GH #392)
    let path_jail = path_jail_outcome();
    board.check(&path_jail);

    // 5b3b) Secret/.env redaction — the exfiltration-defense plane, independent
    // of the jail + shell gating above (#507).
    let secret_detection = secret_detection_outcome();
    board.check(&secret_detection);

    // 5b3c) Managed addon binaries (GH #725): receipt path + sha256 pin +
    // revocation, so binhash-gate refusals surface here instead of at first
    // tool call. Silent when nothing is managed.

    // 5b3d) Managed ONNX Runtime (GH #732): present + readable on
    // embedding-enabled builds. Silent when not provisioned (opt-in).
    if let Some(managed_ort) = managed_ort_outcome() {
        board.check(&managed_ort);
    }

    // 5b4) Cognition v2 activation (science subsystems wired + active)
    let cognition = cognition_activity_outcome();
    board.check(&cognition);

    // 5c) Compact-format passthrough (preserve already-compact TOON output, #342)
    let passthrough_outcome = compact_format_passthrough_outcome();
    board.check(&passthrough_outcome);

    // 6) Proxy upstreams
    let proxy_outcome = proxy_upstream_outcome();
    board.check(&proxy_outcome);

    // 7) Leftover shell hook from an older install
    let shell_files = legacy_shell_hook_outcome();
    board.check(&shell_files);

    // 8) MCP
    let mcp = mcp_config_outcome();
    board.check(&mcp);
    let user_scope_mcp_locations = dirs::home_dir()
        .map(|home| lean_ctx_mcp_location_names(&home))
        .unwrap_or_default();

    // 7b) WSL2 + VS Code: surface the upstream first-call invoke race (GH #669)
    if let Some(wsl_hint) = wsl_vscode_mcp_outcome() {
        board.check(&wsl_hint);
    }

    // 8) Workspace-scope MCP (optional; only when a project-local config exists)
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
            println!(
                "  {YELLOW}⚠{RST}  Crash-loop log: {} recent restarts  {DIM}({}){RST}",
                lines.len(),
                display_user_path(&log_path)
            );
        }
    }

    // Plan mode (advisory)
    let plan_outcomes = plan_mode_outcomes();
    for plan_check in &plan_outcomes {
        board.info(plan_check);
    }

    // 9) Session state (project_root + shell_cwd)
    let session_outcome = session_state_outcome();
    board.check(&session_outcome);

    // 12) Build integrity (canary / origin check)
    let integrity = crate::core::integrity::check();
    let integrity_ok = integrity.seed_ok && integrity.origin_ok;
    let integrity_line = if integrity_ok {
        format!(
            "{BOLD}Build origin{RST}  {GREEN}official{RST}  {DIM}{}{RST}",
            integrity.repo
        )
    } else {
        format!(
            "{BOLD}Build origin{RST}  {RED}MODIFIED REDISTRIBUTION{RST}  {YELLOW}pkg={}, repo={}{RST}",
            integrity.pkg_name, integrity.repo
        )
    };
    board.check(&Outcome {
        ok: integrity_ok,
        line: integrity_line,
    });

    // 14) Claude Code instruction truncation guard
    let claude_truncation = claude_truncation_outcome();
    if let Some(ref ct) = claude_truncation {
        board.check(ct);
    }

    // 15) BM25 cache health
    let bm25_health = bm25_cache_health_outcome();
    board.check(&bm25_health);

    // 15-pre) Quarantined corrupt stats.json (#706): the loader preserved a
    // display cache it could not parse — surface it instead of losing history.
    let stats_quarantine = stats_quarantine_outcome();
    board.check(&stats_quarantine);

    // 15a) Semantic index runtime status (state/timing/persistence) for the
    // active project — surfaces a stuck "warming" index (issue #249).
    let semantic_index = semantic_index_outcome();
    if let Some(ref check) = semantic_index {
        board.check(check);
    }

    // 16) Memory
    let mem = memory_outcome();
    board.check(&mem);

    // 18) RAM Guardian
    let ram_outcome = ram_guardian_outcome();
    board.check(&ram_outcome);

    // 19) Capacity warnings (memory stores near limits)
    let cap_warnings = capacity_warnings();
    for cw in &cap_warnings {
        board.check(cw);
    }

    // 19b) Orphaned knowledge stores (deleted projects — reclaimable bloat, #615)
    let orphan_outcome = orphaned_knowledge_outcome();
    board.check(&orphan_outcome);

    // 20) Proxy health
    let proxy_health = proxy_health_outcome();
    board.check(&proxy_health);

    // 20a) Proxy upstream drift (#449): running proxy serves a different upstream
    // than config.toml resolves to (env override masking config). Only surfaces
    // when the proxy is up and actually drifting.
    let upstream_drift = proxy_upstream_drift_outcome();
    if let Some(ref check) = upstream_drift {
        board.check(check);
    }

    // 20) Stale proxy env (ANTHROPIC_BASE_URL pointing to local proxy while proxy is not enabled)
    let stale_env = stale_proxy_env_outcome();
    if let Some(ref check) = stale_env {
        board.check(check);
    }

    // 21) Claude Pro/Max subscription routed through the proxy without an API key
    let subscription_conflict = proxy_subscription_conflict_outcome();
    if let Some(ref check) = subscription_conflict {
        board.check(check);
    }

    // 22) Deprecation register (GL #394): warn about
    // every surface this build deprecates, with replacement and removal floor.
    let deprecation_check = deprecations::deprecations_outcome();
    board.check(&deprecation_check);

    // MCP server CWD warning (informational, only fires when running as MCP)
    let mcp_cwd = mcp_server_cwd_outcome();
    board.check(&mcp_cwd);

    // LSP servers (optional, informational)
    println!("\n  {BOLD}{WHITE}LSP (optional — for ctx_refactor):{RST}");
    let lsp_outcomes = lsp_server_outcomes();
    for lsp_check in &lsp_outcomes {
        board.info(lsp_check);
    }

    // Tool-schema footprint (informational, not scored). There is one surface —
    // the registry — so report its size rather than a tier name. This used to
    // branch on a pinned tier and print it; #415 was already a bug where it
    // claimed "power" right after the wizard chose lean.
    let advertised = crate::server::registry::build_registry().tool_defs().len();
    println!(
        "  {BOLD}Tools{RST}  {WHITE}{advertised} advertised{RST}  \
         {DIM}one surface, every client{RST}"
    );

    // Session cache health (#361): answer "is the cache actually engaging?"
    // without external instrumentation. CEP sessions + the cross-call hit ratio
    // come from the persistent stats store; `verify-cache` proves it live.
    let cep = &crate::core::stats::load().cep;
    let hit_ratio = if cep.total_cache_reads > 0 {
        (cep.total_cache_hits as f64 / cep.total_cache_reads as f64) * 100.0
    } else {
        0.0
    };
    println!(
        "  {BOLD}Session cache{RST}  {WHITE}{} sessions{RST}  {DIM}{}/{} reads cached ({hit_ratio:.0}% hit) · prove: lean-ctx verify-cache{RST}",
        cep.sessions, cep.total_cache_hits, cep.total_cache_reads
    );

    // The board counted exactly what it rendered — the displayed ✓/✗ list and
    // this tally can no longer drift apart (#433).
    let passed = board.passed;
    let total = board.total;
    let needs_attention = total.saturating_sub(passed);
    println!();
    println!("  {BOLD}{WHITE}Summary:{RST}  {GREEN}{passed}{RST}{DIM}/{total}{RST} checks passed");
    if needs_attention > 0 {
        println!(
            "  {YELLOW}{needs_attention} check(s) need attention.{RST}  Auto-repair what's fixable:  {BOLD}lean-ctx doctor --fix{RST}"
        );
    } else {
        println!("  {GREEN}Everything looks good.{RST}");
    }
    println!("  {DIM}LSP servers are optional enhancements (not counted in score){RST}");
    println!("  {DIM}{}{RST}", crate::core::integrity::origin_line());

    // Refresh the cached latest-version in the background and, if the running
    // binary is behind, nudge toward the fast self-updater right where a
    // confused user looks when something seems off (the "stuck updating"
    // report). Notify-only — never auto-installs.

    needs_attention
}

pub fn run_cli(args: &[String]) -> i32 {
    let (sub, rest) = match args.first().map(String::as_str) {
        Some("integrations") => ("integrations", &args[1..]),
        Some("overhead") => ("overhead", &args[1..]),
        Some("lint-context") => ("lint-context", &args[1..]),
        _ => ("", args),
    };

    let fix = rest.iter().any(|a| a == "--fix");
    let json = rest.iter().any(|a| a == "--json");
    let gate = rest.iter().any(|a| a == "--gate");
    let help = rest.iter().any(|a| a == "--help" || a == "-h");

    if help {
        println!("Usage:");
        println!("  lean-ctx doctor");
        println!(
            "  lean-ctx doctor overhead [--json] [--gate]   Fixed context cost per session (--gate: non-zero exit when over the 8000-token budget)"
        );
        println!(
            "  lean-ctx doctor lint-context [--json]   Lint injected context for low-signal/dup lines"
        );
        println!("  lean-ctx doctor integrations [--json]");
        println!("  lean-ctx doctor --fix [--json]");
        return 0;
    }

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
        // Non-zero exit when checks need attention so `lean-ctx doctor` works
        // as a CI/health gate, not just a pretty printer.
        return i32::from(run() > 0);
    }

    match fix::run_fix(&fix::DoctorFixOptions { json }) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("doctor --fix failed: {e}");
            2
        }
    }
}

pub fn compact_score() -> (u32, u32) {
    let mut passed = 0u32;
    let total = 5u32;

    if resolve_lean_ctx_binary().is_some() || path_in_path_env() {
        passed += 1;
    }
    let lean_dir = crate::core::data_dir::lean_ctx_data_dir().ok();
    if lean_dir.as_ref().is_some_and(|p| p.is_dir()) {
        passed += 1;
    }
    if lean_dir
        .as_ref()
        .map(|d| d.join("stats.json"))
        .and_then(|p| std::fs::metadata(p).ok())
        .is_some_and(|m| m.is_file())
    {
        passed += 1;
    }
    if legacy_shell_hook_outcome().ok {
        passed += 1;
    }
    if mcp_config_outcome().ok {
        passed += 1;
    }

    (passed, total)
}

pub(super) fn print_compact_status(passed: u32, total: u32) {
    let status = if passed == total {
        format!("{GREEN}✓ All {total} checks passed{RST}")
    } else {
        format!("{YELLOW}{passed}/{total} passed{RST} — run {BOLD}lean-ctx doctor{RST} for details")
    };
    println!("  {status}");
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
