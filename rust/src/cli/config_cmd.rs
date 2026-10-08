use crate::core::config;

pub fn cmd_config(args: &[String]) {
    let cfg = config::Config::load();

    if args.is_empty() {
        println!("{}", cfg.show());
        println!(
            "\nChange a key with `lean-ctx config set <key> <value>`; `lean-ctx config init`\nwrites the file with every key documented."
        );
        return;
    }

    match args[0].as_str() {
        "init" | "create" => init_config(),
        "set" => {
            if args.len() < 3 {
                eprintln!("Usage: lean-ctx config set <key> <value>");
                std::process::exit(1);
            }
            let key = &args[1];
            let val = &args[2];

            // Upstream URLs are normalized before the single governed write
            // path below, so the #852 review covers every route.
            let (write_key, write_val): (String, String) = match key.as_str() {
                k if k.starts_with("proxy.upstreams.") => {
                    let effective = normalize_optional_upstream(val).unwrap_or_default();
                    (key.clone(), effective)
                }
                _ => (key.clone(), val.clone()),
            };

            write_config_key(&write_key, &write_val, key, val, args);
        }
        "schema" => {
            let schema = config::schema::ConfigSchema::generate();
            println!(
                "{}",
                serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".to_string())
            );
        }
        "validate" => {
            cmd_validate();
        }
        "show" | "effective" => {
            cmd_show_effective();
        }
        "path" | "which" => {
            cmd_config_path();
        }
        "apply" | "reload" => {
            cmd_apply();
        }
        _ => {
            eprintln!("Usage: lean-ctx config [init|set|show|schema|validate|apply|path]");
            std::process::exit(1);
        }
    }
}

/// Single governed write path for `config set` (#852).
///
/// `key`/`value` are the resolved pair actually persisted; `display_key`/
/// `display_val` are what the user typed (kept for messaging, e.g. when a value
/// was normalized). Behavior:
/// - **no-op** (current == new): report "unchanged", write nothing.
/// - **consequential key** ([`config::risk`]): print a before→after review + a
///   risk note, then require confirmation or `--yes`; abort if declined.
/// - **routine key**: write directly, as before.
fn write_config_key(key: &str, value: &str, display_key: &str, display_val: &str, args: &[String]) {
    const BOLD: &str = "\x1b[1m";
    const DIM: &str = "\x1b[2m";
    const YELLOW: &str = "\x1b[33m";
    const RST: &str = "\x1b[0m";

    let current = config::setter::current_value(key);

    if current.as_deref() == Some(value) {
        println!("{display_key} is already set to {display_val} — unchanged.");
        return;
    }

    if let Some(risk) = config::risk::classify(key) {
        let before = current.as_deref().unwrap_or("(default)");
        let after = if value.is_empty() { "(default)" } else { value };
        println!("{BOLD}Review change to {display_key}{RST}");
        println!("  {before}  →  {after}");
        println!("  {YELLOW}{}{RST}", risk.note);
        if !super::prompt::confirm(
            &format!("Apply {display_key} = {display_val}?"),
            super::prompt::wants_yes(args),
        ) {
            println!("{DIM}Aborted — {display_key} left unchanged.{RST}");
            return;
        }
    }

    match config::setter::set_by_key(key, value) {
        Ok(_) => println!("Updated {display_key} = {display_val}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// Resolves the [`config::Config`] that `config init` should persist.
///
/// `existing_raw` is the verbatim content of the current `config.toml`. When it holds a non-empty, parseable document we
/// return its deserialized form so every user value is retained; an empty/absent
/// file yields defaults, and an unparseable one is surfaced as an error so the
/// caller can refuse to clobber it.
///
/// This is the regression guard for #443: `config init` once wrote
/// `Config::default()`, and `Config::save()` overwrites any key present in both
/// the incoming document and the file (see `config_io::merge_table`), which
/// silently reset customized values like `search_roots` or `compression_level`.
fn config_for_init(existing_raw: Option<&str>) -> Result<config::Config, String> {
    match existing_raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        Some(raw) => toml::from_str::<config::Config>(raw).map_err(|e| e.to_string()),
        None => Ok(config::Config::default()),
    }
}

/// Implements `config init`: (re)writes the global config as a fully
/// annotated reference document, seeded with the user's existing values (#443).
///
/// Unlike `save()` (which keeps the file minimal), this emits every key with its
/// documentation. The body is a verbatim serialization of the resolved config,
/// so no customized value is ever lost; an unparseable existing file is refused
/// upstream by [`config_for_init`] rather than clobbered.
fn init_config() {
    let Some(path) = config::Config::path() else {
        eprintln!("Error: cannot determine the config path");
        return;
    };

    let existing_raw = std::fs::read_to_string(&path).ok();

    let cfg = match config_for_init(existing_raw.as_deref()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "Error: refusing to overwrite an unparseable config.toml ({e}).\n  \
                 Fix it manually or run `lean-ctx doctor --fix`, then retry."
            );
            return;
        }
    };

    let schema = config::schema::ConfigSchema::generate();
    let rendered = config::render_annotated_config(&cfg, &schema);

    match crate::config_io::write_atomic_with_backup(&path, &rendered) {
        Ok(()) => println!("Wrote annotated config at {}", path.display()),
        Err(e) => eprintln!("Error: {e}"),
    }
}

fn cmd_apply() {
    use crate::ipc;

    println!("Applying config changes…");

    // 1. Validate config first
    println!("\n[1/4] Validating config…");
    let schema = config::schema::ConfigSchema::generate();
    let known = schema.known_keys();

    if let Some(path) = config::Config::path()
        && path.exists()
        && let Ok(raw) = std::fs::read_to_string(&path)
        && let Ok(table) = raw.parse::<toml::Table>()
    {
        let mut user_keys = Vec::new();
        fn collect_flat(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
            for (k, v) in table {
                let full = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                if let toml::Value::Table(sub) = v {
                    collect_flat(sub, &full, out);
                } else {
                    out.push(full);
                }
            }
        }
        collect_flat(&table, "", &mut user_keys);
        let warnings: Vec<_> = user_keys
            .iter()
            .filter(|uk| {
                !known.contains(uk) && !known.iter().any(|k| uk.starts_with(&format!("{k}.")))
            })
            .collect();
        if warnings.is_empty() {
            println!("  ✓ All config keys valid.");
        } else {
            for w in &warnings {
                eprintln!("  [WARN] Unknown key: {w}");
            }
            eprintln!(
                "  {} unknown key(s) found. Continuing anyway…",
                warnings.len()
            );
        }
    }

    // 2. Restart processes
    println!("\n[2/4] Restarting processes…");
    crate::proxy_autostart::stop();

    let orphans = ipc::process::kill_all_by_name("lean-ctx");
    if orphans > 0 {
        println!("  Terminated {orphans} orphan process(es).");
    }

    std::thread::sleep(std::time::Duration::from_millis(500));

    let remaining = ipc::process::find_pids_by_name("lean-ctx");
    if !remaining.is_empty() {
        for &pid in &remaining {
            let _ = ipc::process::force_kill(pid);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    crate::proxy_autostart::start();

    // 3. Safety checks
    println!("\n[3/4] Running safety checks…");
    println!(
        "  RAM guard: max {}% system",
        crate::core::config::MAX_RAM_PERCENT
    );

    if let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() {
        let sessions_dir = data_dir.join("sessions");
        let session_count = std::fs::read_dir(&sessions_dir)
            .map_or(0, |rd| rd.filter_map(std::result::Result::ok).count());
        println!("  Sessions dir: {session_count} files");
    }

    // 4. Summary
    println!("\n[4/4] Config applied successfully.");
}

/// Print the absolute path of the resolved global `config.toml` (one line,
/// scriptable).
///
/// This is the canonical way to verify that the CLI and the MCP server resolve
/// the *same* config file (GH #594): run `lean-ctx config path` in a terminal
/// and from the editor's MCP context, then compare. Output is the bare path so
/// it can be diffed/captured in scripts and tests; it exits non-zero only when
/// no config directory can be resolved at all.
fn cmd_config_path() {
    let Some(p) = config::Config::path() else {
        eprintln!("Error: cannot resolve a config directory");
        std::process::exit(1);
    };
    println!("{}", p.display());
}

fn cmd_validate() {
    // GH #450: always surface *where* the effective settings come from first, so
    // a "no config" result is never a dead end and a parse error that puts the
    // runtime on defaults is immediately visible.
    print_config_provenance();

    let schema = config::schema::ConfigSchema::generate();
    let known = schema.known_keys();

    let path = match config::Config::path() {
        Some(p) if p.exists() => p,
        Some(p) => {
            println!("[OK] No config.toml at {} — using defaults.", p.display());
            return;
        }
        None => {
            println!("[OK] No config dir resolved — using defaults.");
            return;
        }
    };

    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[ERROR] Cannot read {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    let table: toml::Table = match raw.parse() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[ERROR] Invalid TOML: {e}");
            std::process::exit(1);
        }
    };

    let mut warnings = 0u32;
    let mut validated = 0u32;

    fn collect_keys(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
        for (k, v) in table {
            let full = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                toml::Value::Table(sub) => collect_keys(sub, &full, out),
                toml::Value::Array(arr) => {
                    out.push(full.clone());
                    for item in arr {
                        if let toml::Value::Table(sub) = item {
                            for sk in sub.keys() {
                                out.push(format!("{full}[].{sk}"));
                            }
                        }
                    }
                }
                _ => out.push(full),
            }
        }
    }

    let mut user_keys = Vec::new();
    collect_keys(&table, "", &mut user_keys);

    for uk in &user_keys {
        let base = uk.split("[].").next().unwrap_or(uk);
        let field = uk.rsplit("[].").next().unwrap_or("");
        let check_key = if uk.contains("[].") {
            format!("{base}.{field}")
        } else {
            uk.clone()
        };

        if known.contains(&check_key)
            || known
                .iter()
                .any(|k| check_key.starts_with(&format!("{k}.")))
        {
            validated += 1;
        } else {
            warnings += 1;
            let suggestion = find_closest(&check_key, &known);
            if let Some(sug) = suggestion {
                eprintln!("[WARN] Unknown key '{uk}' -- did you mean '{sug}'?");
            } else {
                eprintln!("[WARN] Unknown key '{uk}' -- this field does not exist");
            }
        }
    }

    let total = validated + warnings;
    if warnings == 0 {
        println!(
            "[OK] All {total} keys validated successfully ({}).",
            path.display()
        );
    } else {
        println!(
            "[RESULT] {validated} of {total} keys validated, {warnings} unknown ({}).",
            path.display()
        );
        std::process::exit(1);
    }
}

/// Print where the settings come from (GH #450): the resolved `config.toml`
/// path, the layout pin and any parse error that silently puts the runtime on
/// defaults.
fn print_config_provenance() {
    let path = config::Config::path();
    println!("Config source:");
    match &path {
        Some(p) if p.exists() => println!("  config.toml:    {} (exists)", p.display()),
        Some(p) => println!(
            "  config.toml:    {} (missing — using defaults)",
            p.display()
        ),
        None => println!("  config.toml:    <no config dir resolved — using defaults>"),
    }

    // A present-but-unparseable file means the runtime silently runs on
    // defaults. Parse into `Config` to match exactly what `load()` rejects.
    let parse_error = path.as_ref().and_then(|p| {
        let raw = std::fs::read_to_string(p).ok()?;
        toml::from_str::<config::Config>(&raw)
            .err()
            .map(|e| e.to_string())
    });
    if let Some(err) = parse_error {
        println!("  [!] parse error: config.toml is unparseable — running on DEFAULTS:");
        println!("                  {err}");
        println!("                  Run `lean-ctx doctor --fix` to repair.");
    }
    println!();
}

/// Closest config key: dotted-path match within distance 3, then a
/// leaf-segment match within distance 2 (`proxy.prt` → `proxy.port`).
/// Distances come from the shared [`crate::core::levenshtein`] helper (#712);
/// the fixed budgets here predate the shared length-scaled one and are kept —
/// dotted keys are long, so a length-scaled budget would over-suggest.
fn find_closest(needle: &str, haystack: &[String]) -> Option<String> {
    use crate::core::levenshtein::levenshtein;
    let mut best: Option<(usize, &str)> = None;
    for candidate in haystack {
        let d = levenshtein(needle, candidate);
        if d <= 3 && (best.is_none() || d < best.unwrap().0) {
            best = Some((d, candidate));
        }
    }
    if best.is_some() {
        return best.map(|(_, s)| s.to_string());
    }
    let leaf = needle.rsplit('.').next().unwrap_or(needle);
    let mut leaf_best: Option<(usize, &str)> = None;
    for candidate in haystack {
        let cand_leaf = candidate.rsplit('.').next().unwrap_or(candidate);
        let d = levenshtein(leaf, cand_leaf);
        if d <= 2 && (leaf_best.is_none() || d < leaf_best.unwrap().0) {
            leaf_best = Some((d, candidate));
        }
    }
    leaf_best.map(|(_, s)| s.to_string())
}

fn normalize_optional_upstream(value: &str) -> Option<String> {
    use crate::core::config::normalize_url_opt;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        None
    } else {
        normalize_url_opt(trimmed)
    }
}

pub fn cmd_benchmark(args: &[String]) {
    use crate::core::benchmark;

    let action = args.first().map_or("run", std::string::String::as_str);

    match action {
        "--help" | "-h" => {
            println!("Usage: lean-ctx benchmark run [path] [--json]");
            println!("       lean-ctx benchmark report [path]");
            println!("       lean-ctx benchmark replay <sessions.json> [--output report.md]");
            println!("       lean-ctx benchmark eval [path] [--json]");
            println!("       lean-ctx benchmark eval-ab [path] [--suite file.ndjson] [--json]");
            println!("       lean-ctx benchmark scorecard [--json] [--output file]");
            println!("       lean-ctx benchmark dual-arm [--json] [--output file]");
        }
        "dual-arm" => {
            let is_json = args.iter().any(|a| a == "--json");
            let output = parse_flag_value(args, "--output");
            match crate::core::scorecard::dual_arm::run_dual_arm() {
                Ok(sc) => {
                    let rendered = if is_json { sc.to_json() } else { sc.to_human() };
                    if let Some(path) = output {
                        if let Err(e) = std::fs::write(&path, &rendered) {
                            eprintln!("Failed to write dual-arm scorecard to {path}: {e}");
                            std::process::exit(1);
                        }
                        eprintln!("Wrote dual-arm scorecard to {path}");
                    } else {
                        print!("{rendered}");
                    }
                }
                Err(e) => {
                    eprintln!("Dual-arm bench failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        "scorecard" => {
            let is_json = args.iter().any(|a| a == "--json");
            let output = parse_flag_value(args, "--output");
            match crate::core::scorecard::run_scorecard() {
                Ok(sc) => {
                    let rendered = if is_json { sc.to_json() } else { sc.to_human() };
                    if let Some(path) = output {
                        if let Err(e) = std::fs::write(&path, &rendered) {
                            eprintln!("Failed to write scorecard to {path}: {e}");
                            std::process::exit(1);
                        }
                        eprintln!("Wrote scorecard to {path}");
                    } else {
                        print!("{rendered}");
                    }
                }
                Err(e) => {
                    eprintln!("Scorecard failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        "eval" => {
            let path = args.get(1).map_or(".", std::string::String::as_str);
            let is_json = args.iter().any(|a| a == "--json");
            let root = std::path::Path::new(path);

            let index = crate::core::bm25_index::BM25Index::build_from_directory(root);
            let cfg = crate::core::hybrid_search::HybridConfig::default();
            let queries = crate::core::eval_harness::generate_self_eval(&index, 50);

            if queries.is_empty() {
                eprintln!("No symbols found — cannot generate eval queries.");
                std::process::exit(1);
            }

            let scorecard = crate::core::eval_harness::run_eval(root, &queries, &index, &cfg);
            if is_json {
                if let Ok(json) = serde_json::to_string_pretty(&scorecard) {
                    println!("{json}");
                }
            } else {
                print!("{scorecard}");
            }
        }
        "eval-ab" => {
            let path = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .map_or(".", std::string::String::as_str);
            let is_json = args.iter().any(|a| a == "--json");
            let root = std::path::Path::new(path);
            if !root.exists() {
                eprintln!("Path does not exist: {path}");
                std::process::exit(1);
            }

            let index = crate::core::bm25_index::BM25Index::build_from_directory(root);
            let cfg = crate::core::hybrid_search::HybridConfig::default();

            let queries = match parse_flag_value(args, "--suite") {
                Some(suite) => {
                    match crate::core::eval_harness::load_suite(std::path::Path::new(&suite)) {
                        Ok(q) => q,
                        Err(e) => {
                            eprintln!("Failed to load suite {suite}: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                None => crate::core::eval_harness::generate_self_eval(&index, 50),
            };

            if queries.is_empty() {
                eprintln!("No eval queries (empty suite / no symbols indexed).");
                std::process::exit(1);
            }

            let report = crate::core::eval_harness::run_ab(root, &queries, &index, &cfg);
            if is_json {
                println!("{}", report.to_json());
            } else {
                print!("{report}");
            }
        }
        "run" => {
            let path = args.get(1).map_or(".", std::string::String::as_str);
            let is_json = args.iter().any(|a| a == "--json");

            let result = benchmark::run_project_benchmark(path);
            if is_json {
                println!("{}", benchmark::format_json(&result));
            } else {
                println!("{}", benchmark::format_terminal(&result));
            }
        }
        "report" => {
            let path = args.get(1).map_or(".", std::string::String::as_str);
            let result = benchmark::run_project_benchmark(path);
            println!("{}", benchmark::format_markdown(&result));
        }
        "replay" => {
            let Some(input) = args.get(1).filter(|arg| !arg.starts_with("--")) else {
                eprintln!("Usage: lean-ctx benchmark replay <sessions.json> [--output report.md]");
                std::process::exit(1);
            };
            let result = crate::core::quality_benchmark::load_replay(std::path::Path::new(input))
                .and_then(|suite| crate::core::quality_benchmark::replay(&suite));
            match result {
                Ok(report) => {
                    let markdown = crate::core::quality_benchmark::format_markdown(&report);
                    if let Some(output) = parse_flag_value(args, "--output") {
                        if let Err(error) = std::fs::write(&output, markdown) {
                            eprintln!("Failed to write benchmark report to {output}: {error}");
                            std::process::exit(1);
                        }
                        eprintln!("Wrote compression quality report to {output}");
                    } else {
                        print!("{markdown}");
                    }
                }
                Err(error) => {
                    eprintln!("Benchmark replay failed: {error:#}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            if std::path::Path::new(action).exists() {
                let result = benchmark::run_project_benchmark(action);
                println!("{}", benchmark::format_terminal(&result));
            } else {
                eprintln!("Usage: lean-ctx benchmark run [path] [--json]");
                eprintln!("       lean-ctx benchmark report [path]");
                eprintln!("       lean-ctx benchmark replay <sessions.json> [--output report.md]");
                eprintln!("       lean-ctx benchmark eval [path] [--json]");
                eprintln!(
                    "       lean-ctx benchmark eval-ab [path] [--suite file.ndjson] [--json]"
                );
                eprintln!("       lean-ctx benchmark scorecard [--json] [--output file]");
                std::process::exit(1);
            }
        }
    }
}

fn parse_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

pub fn cmd_cache(args: &[String]) {
    use crate::core::cli_cache;
    match args.first().map(std::string::String::as_str) {
        Some("clear") => {
            let count = cli_cache::clear();
            println!("Cleared {count} cached entries.");
        }
        Some("reset") => {
            let project_flag = args.get(1).map(std::string::String::as_str) == Some("--project");
            if project_flag {
                let root =
                    crate::core::session::SessionState::load_latest().and_then(|s| s.project_root);
                if let Some(root) = root {
                    let count = cli_cache::clear_project(&root);
                    println!("Reset {count} cache entries for project: {root}");
                } else {
                    eprintln!("No active project root found. Start a session first.");
                    std::process::exit(1);
                }
            } else {
                let count = cli_cache::clear();
                println!("Reset all {count} cache entries.");
            }
        }
        Some("stats") => {
            let (hits, reads, entries) = cli_cache::stats();
            let rate = if reads > 0 {
                (hits as f64 / reads as f64 * 100.0).round() as u32
            } else {
                0
            };
            println!("CLI Cache Stats (lean-ctx read / lean-ctx grep):");
            println!("  Entries:   {entries}");
            println!("  Reads:     {reads}");
            println!("  Hits:      {hits}");
            println!("  Hit Rate:  {rate}%");
        }
        Some("invalidate") => {
            if args.len() < 2 {
                eprintln!("Usage: lean-ctx cache invalidate <path>");
                std::process::exit(1);
            }
            cli_cache::invalidate(&args[1]);
            println!("Invalidated cache for {}", args[1]);
        }
        Some("prune") => {
            let bm25 = prune_bm25_caches();
            let graph = prune_graph_caches();
            // Reclaim knowledge stores whose project_root was deleted (removed
            // worktrees, thrown-away projects): they can never be written again,
            // so their per-store eviction cap can never self-heal — pure bloat (#615).
            let orphans = crate::core::knowledge::maintenance::prune_orphaned_stores();

            let removed = bm25.removed + graph.removed + orphans.removed as u32;
            let freed = bm25.bytes_freed + graph.bytes_freed + orphans.reclaimed_bytes;
            println!(
                "Pruned {} entries, freed {:.1} MB (BM25: {}, graphs: {}, orphaned stores: {})",
                removed,
                freed as f64 / 1_048_576.0,
                bm25.removed,
                graph.removed,
                orphans.removed,
            );
        }
        _ => {
            let (hits, reads, entries) = cli_cache::stats();
            let rate = if reads > 0 {
                (hits as f64 / reads as f64 * 100.0).round() as u32
            } else {
                0
            };
            println!("CLI File Cache: {entries} entries, {hits}/{reads} hits ({rate}%)");
            println!();
            println!("Subcommands:");
            println!("  cache stats       Show detailed stats");
            println!("  cache clear       Clear all cached entries");
            println!("  cache reset       Reset all cache (or --project for current project only)");
            println!("  cache invalidate  Remove specific file from cache");
            println!(
                "  cache prune       Reclaim BM25 + graph indexes and orphaned knowledge stores"
            );
        }
    }
}

pub struct PruneResult {
    pub scanned: u32,
    pub removed: u32,
    pub bytes_freed: u64,
}

pub fn prune_bm25_caches() -> PruneResult {
    let mut result = PruneResult {
        scanned: 0,
        removed: 0,
        bytes_freed: 0,
    };

    let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() else {
        return result;
    };
    let vectors_dir = data_dir.join("vectors");
    let Ok(entries) = std::fs::read_dir(&vectors_dir) else {
        return result;
    };

    let max_bytes = crate::core::config::DEFAULT_BM25_PERSIST_MB * 1024 * 1024;

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        result.scanned += 1;

        {
            let q_name = &"bm25_index.bin.zst.quarantined";
            let quarantined = dir.join(q_name);
            if quarantined.exists() {
                if let Ok(meta) = std::fs::metadata(&quarantined) {
                    result.bytes_freed += meta.len();
                }
                let _ = std::fs::remove_file(&quarantined);
                result.removed += 1;
                println!("  Removed quarantined: {}", quarantined.display());
            }
        }

        let index_path = dir.join("bm25_index.bin.zst");
        if let Ok(meta) = std::fs::metadata(&index_path)
            && meta.len() > max_bytes
        {
            result.bytes_freed += meta.len();
            let _ = std::fs::remove_file(&index_path);
            result.removed += 1;
            println!(
                "  Removed oversized ({:.1} MB): {}",
                meta.len() as f64 / 1_048_576.0,
                index_path.display()
            );
        }

        let marker = dir.join("project_root.txt");
        if let Ok(root_str) = std::fs::read_to_string(&marker) {
            let root_path = std::path::Path::new(root_str.trim());
            if !root_path.exists() {
                let freed = dir_size(&dir);
                result.bytes_freed += freed;
                let _ = std::fs::remove_dir_all(&dir);
                result.removed += 1;
                println!(
                    "  Removed orphaned ({:.1} MB, project gone: {}): {}",
                    freed as f64 / 1_048_576.0,
                    root_str.trim(),
                    dir.display()
                );
            }
        }
    }

    result
}

pub fn prune_graph_caches() -> PruneResult {
    let mut result = PruneResult {
        scanned: 0,
        removed: 0,
        bytes_freed: 0,
    };

    let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() else {
        return result;
    };
    let graphs_dir = data_dir.join("graphs");
    let Ok(entries) = std::fs::read_dir(&graphs_dir) else {
        return result;
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        result.scanned += 1;

        // #696 C4: the property graph (graph.db + graph.meta.json) is the sole
        // store. The meta carries the absolute project root, so an orphaned
        // `graphs/<hash>/` dir (project deleted) can still be pruned.
        let meta_file = dir.join("graph.meta.json");
        let db_file = dir.join("graph.db");
        if !meta_file.exists() && !db_file.exists() {
            continue;
        }

        let root_from_meta = try_read_project_root_from_graph(&meta_file);
        if let Some(root) = root_from_meta
            && !root.is_empty()
            && !std::path::Path::new(&root).exists()
        {
            let freed = dir_size(&dir);
            result.bytes_freed += freed;
            let _ = std::fs::remove_dir_all(&dir);
            result.removed += 1;
            println!(
                "  Removed orphaned graph ({:.1} MB, project gone: {}): {}",
                freed as f64 / 1_048_576.0,
                root,
                dir.display()
            );
            continue;
        }

        // Oversized guard: a pathologically large (e.g. corrupt) graph store is
        // dropped so the next query rebuilds it cleanly — a rebuild cost, not
        // data loss.
        if let Ok(meta) = std::fs::metadata(&db_file)
            && meta.len() > 100 * 1024 * 1024
        {
            let freed = dir_size(&dir);
            result.bytes_freed += freed;
            let _ = std::fs::remove_dir_all(&dir);
            result.removed += 1;
            println!(
                "  Removed oversized graph ({:.1} MB): {}",
                freed as f64 / 1_048_576.0,
                dir.display()
            );
        }
    }

    result
}

/// Read the absolute project root recorded in a `graph.meta.json` file, if
/// present (#696 C4 — replaces reading it from the retired JSON index).
fn try_read_project_root_from_graph(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("project_root")?.as_str().map(String::from)
}

fn cmd_show_effective() {
    let cfg = config::Config::load();
    let compression = config::CompressionLevel::effective(&cfg);
    let policy = crate::core::memory_policy::MemoryPolicy::default();

    println!("{}", box_top("Settings"));
    box_row(&format!(
        " compression_level   = {:10}  {}",
        format!("{:.2} ({compression:?})", cfg.compression_level),
        source_hint(!crate::core::config::is_default_compression_level_value(
            cfg.compression_level
        ))
    ));
    println!("{}", box_bottom());

    println!();
    println!("{}", box_top("Built-in limits"));
    box_row(&format!(
        " BM25 index cap          {:>6} MB",
        crate::core::config::DEFAULT_BM25_PERSIST_MB
    ));
    box_row(&format!(
        " graph index file cap    {:>6}",
        crate::core::config::GRAPH_INDEX_MAX_FILES
    ));
    box_row("");
    box_row(&format!(
        " knowledge facts cap     {:>6}",
        policy.knowledge.max_facts
    ));
    box_row(&format!(
        " knowledge patterns cap  {:>6}",
        policy.knowledge.max_patterns
    ));
    println!("{}", box_bottom());
}

/// Interior width of the `config show` frames, in terminal columns.
const SHOW_BOX_W: usize = 60;

/// `╭─── Label ───…───╮`, always `SHOW_BOX_W` columns wide inside the corners.
fn box_top(label: &str) -> String {
    let head = format!("─── {label} ");
    let fill = SHOW_BOX_W.saturating_sub(crate::core::theme::visual_len(&head));
    format!("╭{head}{}╮", "─".repeat(fill))
}

fn box_bottom() -> String {
    format!("╰{}╯", "─".repeat(SHOW_BOX_W))
}

/// Print one framed row, padded (or truncated) to the frame's interior width so
/// the right border lines up regardless of the value's length.
fn box_row(content: &str) {
    println!("│{}│", crate::core::theme::pad_right(content, SHOW_BOX_W));
}

fn source_hint(config_set: bool) -> &'static str {
    if config_set {
        "← config"
    } else {
        "← default"
    }
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                total += std::fs::metadata(&p).map_or(0, |m| m.len());
            } else if p.is_dir() {
                total += dir_size(&p);
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_box_borders_line_up() {
        use crate::core::theme::{pad_right, visual_len};
        let bottom = visual_len(&box_bottom());
        for label in ["Simplified (high-level)", "Derived effective limits", ""] {
            assert_eq!(visual_len(&box_top(label)), bottom, "top border: {label:?}");
        }
        // Rows track the same width, short and overlong alike.
        for row in ["", " x = 1", &" long ".repeat(40)] {
            assert_eq!(visual_len(&pad_right(row, SHOW_BOX_W)) + 2, bottom);
        }
    }

    // Reproduces `Config::save()`'s on-disk merge without touching the real
    // config path: serialize `cfg`, then merge it onto `existing` exactly as
    // save() does, and return the value that `shell_timeout_secs` ends up with.
    fn merged_shell_timeout(cfg: &config::Config, existing: &str) -> Option<u64> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, existing).unwrap();
        let new_content = toml::to_string_pretty(cfg).unwrap();
        let baseline = toml::from_str::<config::Config>("").unwrap();
        let defaults = toml::to_string_pretty(&baseline).unwrap();
        crate::config_io::write_toml_preserving_minimal(&path, &new_content, &defaults).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        toml::from_str::<config::Config>(&written)
            .unwrap()
            .shell_timeout_secs
    }

    #[test]
    fn init_uses_existing_values_not_defaults() {
        let existing = "shell_timeout_secs = 90\ncompression_level = 0.5\n";
        let cfg = config_for_init(Some(existing)).expect("parse existing");
        assert_eq!(
            cfg.shell_timeout_secs,
            Some(90),
            "must keep the user's value"
        );
        assert_eq!(cfg.compression_level, 0.5);
    }

    #[test]
    fn init_falls_back_to_defaults_on_fresh_install() {
        let cfg = config_for_init(None).expect("default");
        assert_eq!(
            cfg.shell_timeout_secs,
            config::Config::default().shell_timeout_secs
        );
        assert_eq!(
            cfg.compression_level,
            config::Config::default().compression_level
        );
        let cfg_empty = config_for_init(Some("   \n")).expect("blank -> default");
        assert_eq!(
            cfg_empty.shell_timeout_secs,
            config::Config::default().shell_timeout_secs
        );
    }

    #[test]
    fn init_refuses_unparseable_config() {
        assert!(config_for_init(Some("shell_timeout_secs = = =")).is_err());
    }

    // #443 end-to-end: `config init` must not reset a customized value.
    #[test]
    fn init_preserves_value_through_save_merge() {
        let existing = "shell_timeout_secs = 90\n";
        let cfg = config_for_init(Some(existing)).unwrap();
        assert_eq!(
            merged_shell_timeout(&cfg, existing),
            Some(90),
            "user value must survive `config init`"
        );
    }
}
