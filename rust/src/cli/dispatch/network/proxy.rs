//! `lean-ctx proxy …` — proxy lifecycle, upstream stats.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Parse `--port=N` from proxy args, falling back to the configured default.
fn parse_proxy_port(rest: &[String]) -> u16 {
    rest.iter()
        .find_map(|p| p.strip_prefix("--port="))
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(crate::proxy_setup::default_port)
}

/// Stops a standalone/foreground proxy by reading its PID from `/health` and
/// terminating it (graceful, then force). Returns true if a proxy was reachable
/// on `port`, false if nothing was listening. Shared by `stop` and `restart`.
fn stop_proxy_process(port: u16) -> bool {
    let health_url = format!("http://127.0.0.1:{port}/health");
    let Ok(resp) = ureq::get(&health_url).call() else {
        return false;
    };
    let pid = resp.into_body().read_to_string().ok().and_then(|body| {
        body.split("pid\":")
            .nth(1)
            .and_then(|s| s.split([',', '}']).next())
            .and_then(|s| s.trim().parse::<u32>().ok())
    });
    match pid {
        Some(pid) => {
            let _ = crate::ipc::process::terminate_gracefully(pid);
            std::thread::sleep(std::time::Duration::from_millis(500));
            if crate::ipc::process::is_alive(pid) {
                let _ = crate::ipc::process::force_kill(pid);
            }
            println!("Proxy on port {port} stopped (PID {pid}).");
        }
        None => {
            println!(
                "Proxy on port {port} running but could not parse PID. Use `lean-ctx stop` to kill all."
            );
        }
    }
    true
}

fn print_compression_by_upstream(v: &serde_json::Value) {
    let Some(per_upstream) = v.get("per_upstream").and_then(|u| u.as_object()) else {
        return;
    };
    println!("  Compression by upstream:");
    for (label, key) in [
        ("Anthropic", "anthropic"),
        ("OpenAI", "openai"),
        ("Gemini", "gemini"),
        ("Grok", "grok"),
        ("CommandCode", "commandcode"),
    ] {
        let Some(row) = per_upstream.get(key).and_then(|x| x.as_object()) else {
            continue;
        };
        let requests = row
            .get("requests_total")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let saved = row
            .get("tokens_saved")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let ratio = row
            .get("compression_ratio_pct")
            .and_then(|x| x.as_str())
            .unwrap_or("0.0");
        println!("    {label:<10} {ratio:>5}% saved, {saved} tok, {requests} req");
    }
}

/// Prints the provider-verified savings line (#701) when counterfactual
/// metering has covered at least one request. Both sides of the pair were
/// counted by Anthropic on the same request — receipts, not the bytes/4
/// estimate above. Silent when the opt-in feature is off or no probe has
/// answered yet (`verified_savings` is `null`).
fn print_verified_savings(v: &serde_json::Value) {
    let Some(vs) = v.get("verified_savings").filter(|x| x.is_object()) else {
        return;
    };
    let saved = vs
        .get("verified_saved_tokens")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let requests = vs
        .get("requests")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let counterfactual = vs
        .get("counterfactual_input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    #[allow(clippy::cast_precision_loss)]
    let pct = if counterfactual > 0 {
        saved as f64 * 100.0 / counterfactual as f64
    } else {
        0.0
    };
    println!(
        "  Verified:    {saved} tok saved across {requests} probed req ({pct:.1}% — provider-counted, #701)"
    );
}

/// Prints the proxy's live upstreams (from `/status`) and warns when they drift
/// from what the operator expects. Covers both #449 cases: a shell-exported
/// `LEAN_CTX_*_UPSTREAM` that never reached the MCP/service-spawned proxy, and a
/// proxy started with an env override that now masks a later config.toml edit.
fn print_live_upstreams_and_drift(v: &serde_json::Value, cfg: &crate::core::config::Config) {
    use crate::core::config::{
        ProxyProvider, UpstreamDrift, diagnose_drift, env_upstream_override,
    };

    let Some(up) = v.get("upstreams").and_then(|u| u.as_object()) else {
        return;
    };
    let disk = cfg.proxy.resolve_all_disk();
    println!("  Upstreams (live):");
    let mut notes = Vec::new();
    for (label, key, provider, disk_val) in [
        (
            "Anthropic",
            "anthropic",
            ProxyProvider::Anthropic,
            &disk.anthropic,
        ),
        ("OpenAI", "openai", ProxyProvider::OpenAi, &disk.openai),
        ("Gemini", "gemini", ProxyProvider::Gemini, &disk.gemini),
    ] {
        let live = up.get(key).and_then(|x| x.as_str()).unwrap_or("?");
        println!("    {label:<10} {live}");
        if live == "?" {
            continue;
        }
        let env = env_upstream_override(provider);
        match diagnose_drift(env.as_deref(), disk_val, live) {
            Some(UpstreamDrift::EnvNotApplied) => {
                let want = env.as_deref().unwrap_or("");
                notes.push(format!(
                    "  \x1b[33m⚠ {label}: LEAN_CTX_{}_UPSTREAM is set in this shell ({want})\x1b[0m\n  \
                       \x1b[33m  but the running proxy serves {live}. Environment variables do not reach\x1b[0m\n  \
                       \x1b[33m  an MCP/service-spawned proxy (#449). Persist it — applies live:\x1b[0m\n  \
                       \x1b[33m    lean-ctx config set proxy.{key}_upstream {want}\x1b[0m",
                    label.to_uppercase(),
                ));
            }
            Some(UpstreamDrift::ConfigNotApplied) => {
                notes.push(format!(
                    "  \x1b[33m⚠ {label}: proxy serves {live} but config.toml resolves to {disk_val}.\x1b[0m\n  \
                       \x1b[33m  Apply it: lean-ctx proxy restart\x1b[0m",
                ));
            }
            None => {}
        }
    }
    // Registry providers (enterprise#7): Grok dual-rail and other
    // `[[proxy.providers]]` entries only appear under `/providers/{id}/…`.
    if let Some(providers) = v.get("providers").and_then(|p| p.as_array()) {
        for p in providers {
            let id = p.get("id").and_then(|x| x.as_str()).unwrap_or("?");
            let base = p.get("base_url").and_then(|x| x.as_str()).unwrap_or("?");
            let label = if id.eq_ignore_ascii_case("grok-chat")
                || id.eq_ignore_ascii_case("xai")
                || id.eq_ignore_ascii_case("grok")
            {
                "Grok"
            } else if id.eq_ignore_ascii_case("commandcode")
                || id.eq_ignore_ascii_case("command-code")
            {
                "CommandCode"
            } else {
                id
            };
            // Prefer a stable display name; still show registry id when it
            // differs (e.g. grok-chat vs xai).
            let canonical_id = match label {
                "Grok" => Some("grok"),
                "CommandCode" => Some("commandcode"),
                _ => None,
            };
            if let Some(canon) = canonical_id
                && !id.eq_ignore_ascii_case(canon)
            {
                println!("    {label:<10} {base}  [{id}]");
            } else {
                println!("    {label:<10} {base}");
            }
        }
    }
    for note in notes {
        println!();
        println!("{note}");
    }

    // #590: a configured custom HTTPS upstream that is not permitted is silently
    // ignored — the managed proxy can't see the shell's env opt-in, so it serves
    // the provider default. Point the operator at the *persistent* config flag,
    // which does reach the managed proxy. Suppressed once the opt-in is active
    // (env or config), since the drift notes above then cover any remaining gap.
    if cfg.proxy.has_custom_host_upstream() && !cfg.proxy.allows_custom_upstream() {
        println!();
        println!(
            "  \x1b[33m⚠ A custom upstream host is configured but not permitted, so the proxy\x1b[0m"
        );
        println!(
            "  \x1b[33m  serves the provider default. Allow it (reaches the managed proxy):\x1b[0m"
        );
        println!("  \x1b[33m    lean-ctx config set proxy.allow_custom_upstream true\x1b[0m");
        println!("  \x1b[33m    lean-ctx proxy restart\x1b[0m");
    }
}

/// Bridges the shell's `LEAN_CTX_ALLOW_CUSTOM_UPSTREAM` opt-in into `config.toml`
/// so the managed (LaunchAgent / systemd) proxy — which only reads `config.toml`,
/// never the shell env — honors a configured custom upstream (#590).
///
/// No-op unless all three hold: the env opt-in is present, a custom-host upstream
/// is actually configured, and `[proxy] allow_custom_upstream` is not already
/// `true` (idempotent). Returns true when it persisted the flag.
fn bridge_custom_upstream_optin() -> bool {
    if std::env::var("LEAN_CTX_ALLOW_CUSTOM_UPSTREAM").is_err() {
        return false;
    }
    let cfg = crate::core::config::Config::load();
    if cfg.proxy.allow_custom_upstream == Some(true) || !cfg.proxy.has_custom_host_upstream() {
        return false;
    }
    match crate::core::config::Config::update_global(|c| {
        c.proxy.allow_custom_upstream = Some(true);
    }) {
        Ok(_) => {
            println!(
                "  \x1b[32m✓\x1b[0m Custom upstream opt-in persisted: [proxy] allow_custom_upstream = true"
            );
            println!(
                "  \x1b[2m  (so the managed proxy honors your custom upstream — the env var never reaches it, #590)\x1b[0m"
            );
            true
        }
        Err(e) => {
            tracing::warn!("could not persist allow_custom_upstream: {e}");
            false
        }
    }
}

pub(crate) fn cmd_proxy(rest: &[String]) {
    {
        // `--help` anywhere must never execute the verb (GH #393).
        if wants_help(rest) {
            println!(
                "Usage: lean-ctx proxy <start|stop|restart|status|enable|disable|cleanup|token> [--port=4444]"
            );
            println!();
            println!("Commands:");
            println!(
                "  start     Run the compression proxy (foreground; -d/--detach for background; --autostart installs a service)"
            );
            println!("  stop      Stop the proxy on the given port");
            println!(
                "  restart   Restart the managed proxy (re-reads config.toml; drops env overrides)"
            );
            println!("  status    Show proxy config, process, live upstreams and stats");
            println!("  enable    Enable the proxy: config flag, autostart service, env wiring");
            println!("  disable   Disable the proxy and restore the original endpoint");
            println!("  cleanup   Remove stale proxy URLs from AI tool configs");
            println!("  token     Print the current proxy Bearer token (for MCP/HTTP clients)");
            return;
        }
        let sub = rest.first().map_or("help", std::string::String::as_str);
        match sub {
            "start" => {
                let port: u16 = rest
                    .iter()
                    .find_map(|p| p.strip_prefix("--port=").or_else(|| p.strip_prefix("-p=")))
                    .and_then(|p| p.parse().ok())
                    .unwrap_or_else(crate::proxy_setup::default_port);
                let autostart = rest.iter().any(|a| a == "--autostart");
                if autostart {
                    if !crate::proxy_autostart::install(port, false) {
                        std::process::exit(1);
                    }
                    return;
                }
                let detach = rest.iter().any(|a| a == "--detach" || a == "-d");
                if detach {
                    start_detached(port);
                    return;
                }
                if let Err(e) = crate::cli::dispatch::run_async(crate::proxy::start_proxy(port)) {
                    tracing::error!("Proxy error: {e}");
                    std::process::exit(1);
                }
            }
            "stop" => {
                let port = parse_proxy_port(rest);
                if !stop_proxy_process(port) {
                    println!("No proxy running on port {port}.");
                }
            }
            "restart" => {
                let port = parse_proxy_port(rest);
                if crate::proxy_autostart::is_installed() {
                    // #590: persist the shell's custom-upstream opt-in to config
                    // before the restart so the re-read picks up the custom host.
                    bridge_custom_upstream_optin();
                    // Managed service (LaunchAgent / systemd): a clean bootout +
                    // bootstrap restarts the proxy so it re-reads config.toml. It
                    // deliberately drops any `LEAN_CTX_*_UPSTREAM` env override
                    // (the service context has none), making config.toml the
                    // single source of truth for the long-lived proxy (#449).
                    crate::proxy_autostart::stop();
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    if crate::proxy_autostart::start_on_port(port) {
                        println!("\x1b[32m✓\x1b[0m Proxy restarted (managed service).");
                        println!("  Verify active upstreams: lean-ctx proxy status");
                    } else {
                        eprintln!(
                            "\x1b[31m✗\x1b[0m Managed proxy restart failed; port {port} was not transferred."
                        );
                        std::process::exit(1);
                    }
                } else if stop_proxy_process(port) {
                    println!();
                    println!("  No autostart service installed — start the proxy again:");
                    println!("    lean-ctx proxy start --port={port}");
                } else {
                    println!("No proxy running on port {port} and no autostart service installed.");
                    println!("  Start it now:       lean-ctx proxy start --port={port}");
                    println!("  Or install service: lean-ctx proxy enable");
                }
            }
            "status" => {
                let port = parse_proxy_port(rest);
                let cfg = crate::core::config::Config::load();
                println!("lean-ctx proxy:");
                match cfg.proxy_enabled {
                    Some(true) => println!("  Config:  enabled"),
                    Some(false) => println!("  Config:  disabled"),
                    None => println!("  Config:  undecided (not yet configured)"),
                }
                println!("  Port:    {port}");
                // Liveness comes from the *public* /health endpoint so a running
                // proxy is never misreported as down — even mid-upgrade when the
                // managed proxy still holds an old session token (#449). The rich
                // detail (stats + live upstreams) comes from the authenticated
                // /status; if that 401s while /health is up, we still report it as
                // running and point at `proxy restart`.
                let alive = ureq::get(&format!("http://127.0.0.1:{port}/health"))
                    .call()
                    .is_ok();
                if alive {
                    println!("  Process: running");
                    let token =
                        crate::core::session_token::resolve_proxy_token("LEAN_CTX_PROXY_TOKEN");
                    let status = ureq::get(&format!("http://127.0.0.1:{port}/status"))
                        .header("Authorization", &format!("Bearer {token}"))
                        .call();
                    if let Ok(resp) = status {
                        let body = resp.into_body().read_to_string().unwrap_or_default();
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                            println!("  Requests:    {}", v["requests_total"]);
                            println!("  Compressed:  {}", v["requests_compressed"]);
                            println!("  Tokens saved: {} (estimated)", v["tokens_saved"]);
                            println!(
                                "  Compression: {}%",
                                v["compression_ratio_pct"].as_str().unwrap_or("0.0")
                            );
                            print_verified_savings(&v);
                            print_compression_by_upstream(&v);
                            print_live_upstreams_and_drift(&v, &cfg);
                        }
                    } else {
                        println!(
                            "  \x1b[33m⚠ Live details unavailable: the running proxy rejects this\x1b[0m"
                        );
                        println!(
                            "  \x1b[33m  shell's session token. Re-sync it: lean-ctx proxy restart\x1b[0m"
                        );
                    }
                } else {
                    println!("  Process: not running");
                }
                if cfg.proxy_enabled == Some(false) || cfg.proxy_enabled.is_none() {
                    println!();
                    println!("  Enable: lean-ctx proxy enable");

                    let home = dirs::home_dir().unwrap_or_default();
                    if crate::proxy_setup::has_stale_proxy_url(&home) {
                        println!();
                        println!(
                            "  \x1b[33m⚠ WARNING: Claude Code ANTHROPIC_BASE_URL points to the local proxy,\x1b[0m"
                        );
                        println!(
                            "  \x1b[33m  but proxy is not enabled. This causes 401 auth failures.\x1b[0m"
                        );
                        println!("  Fix:  lean-ctx proxy cleanup   (remove stale URL)");
                        println!("        lean-ctx proxy enable    (enable the proxy)");
                    }
                }
            }
            "enable" => {
                let force = rest.iter().any(|a| a == "--force");
                if let Err(e) =
                    crate::core::config::Config::update_global(|c| c.proxy_enabled = Some(true))
                {
                    tracing::warn!("could not persist proxy_enabled: {e}");
                }

                // #590: persist the shell's custom-upstream opt-in to config BEFORE
                // the managed proxy starts, so it reads the flag on startup (the
                // service never inherits the shell's env var).
                bridge_custom_upstream_optin();

                let port = crate::proxy_setup::default_port();
                if !crate::proxy_autostart::install(port, false) {
                    eprintln!(
                        "\x1b[31m✗\x1b[0m Proxy enable failed; managed service did not acquire port {port}."
                    );
                    std::process::exit(1);
                }
                std::thread::sleep(std::time::Duration::from_millis(500));

                let home = dirs::home_dir().unwrap_or_default();
                crate::proxy_setup::install_proxy_env_unchecked(&home, port, false, force);
                println!(
                    "\x1b[32m✓\x1b[0m Proxy enabled on port {port}. LLM requests will be compressed."
                );
            }
            "disable" => {
                if let Err(e) =
                    crate::core::config::Config::update_global(|c| c.proxy_enabled = Some(false))
                {
                    tracing::warn!("could not persist proxy_enabled: {e}");
                }

                crate::proxy_autostart::uninstall(false);
                let home = dirs::home_dir().unwrap_or_default();
                crate::proxy_setup::uninstall_proxy_env(&home, false);

                println!("\x1b[32m✓\x1b[0m Proxy disabled. Original endpoint restored.");
                println!("  Re-enable anytime: lean-ctx proxy enable");
            }
            "cleanup" => {
                let home = dirs::home_dir().unwrap_or_default();
                let removed = crate::proxy_setup::cleanup_stale_proxy_env(&home);
                if removed > 0 {
                    println!("\x1b[32m✓\x1b[0m Cleaned up {removed} stale proxy URL(s).");
                    println!("  Restart your AI tool for changes to take effect.");
                } else {
                    println!("  No stale proxy URLs found. Nothing to clean up.");
                }
            }
            "token" => {
                let token = crate::core::session_token::resolve_proxy_token("LEAN_CTX_PROXY_TOKEN");
                let quiet = rest.iter().any(|a| a == "--quiet" || a == "-q");
                if quiet {
                    print!("{token}");
                } else {
                    println!("{token}");
                }
            }
            _ => {
                println!(
                    "Usage: lean-ctx proxy <start|stop|restart|status|enable|disable|cleanup|token> [--port=4444]"
                );
            }
        }
    }
    /// Fork a child process running `lean-ctx proxy start --port=N` with
    /// stdout/stderr redirected to a log file, then exit the parent immediately.
    /// The child's PID is written to the proxy PID file so `lean-ctx proxy stop`
    /// can find and kill it (#758).
    fn start_detached(port: u16) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "lean-ctx".into());
        let log_dir = crate::core::paths::state_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("lean-ctx"));
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("proxy.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap_or_else(|e| {
                eprintln!("Cannot open {}: {e}", log_path.display());
                std::process::exit(1);
            });
        let stderr_file = log_file.try_clone().unwrap_or_else(|e| {
            eprintln!("Cannot clone log fd: {e}");
            std::process::exit(1);
        });
        match std::process::Command::new(&exe)
            .args(["proxy", "start", &format!("--port={port}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log_file))
            .stderr(std::process::Stdio::from(stderr_file))
            .spawn()
        {
            Ok(child) => {
                println!(
                    "\x1b[32m✓\x1b[0m Proxy started in background (PID {}, port {port})",
                    child.id()
                );
                println!("  Logs: {}", log_path.display());
                println!("  Stop: lean-ctx proxy stop --port={port}");
            }
            Err(e) => {
                eprintln!("Failed to start detached proxy: {e}");
                std::process::exit(1);
            }
        }
    }
}
