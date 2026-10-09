//! `sobre proxy …` — proxy lifecycle and live upstreams.

#[allow(clippy::wildcard_imports)]
use super::*;

/// The port named by `--port N`, `--port=N`, `-p N` or `-p=N`. A missing or
/// unparseable value is a usage error (exit 2).
fn port_flag(rest: &[String]) -> Option<u16> {
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--port" | "-p" => args.next().map(String::as_str),
            a => match a.strip_prefix("--port=").or_else(|| a.strip_prefix("-p=")) {
                Some(v) => Some(v),
                None => continue,
            },
        };
        if let Some(port) = value.and_then(|v| v.parse::<u16>().ok()) {
            return Some(port);
        }
        crate::errln!("Error: {arg} needs a port number (1-65535).");
        std::process::exit(2);
    }
    None
}

/// The port to talk to: `--port`, else the last port a proxy was started on,
/// else the configured one.
fn proxy_port(rest: &[String]) -> u16 {
    port_flag(rest)
        .or_else(crate::proxy_setup::last_started_port)
        .unwrap_or_else(crate::proxy_setup::default_port)
}

const USAGE: &str =
    "Usage: sobre proxy <start|stop|restart|status|enable|disable|cleanup|token> [--port N]";

/// Stops a standalone/foreground proxy identified by its `/health` answer.
/// Returns true if a sobre proxy was answering on `port`. Shared by `stop` and
/// `restart`.
fn stop_proxy_process(port: u16) -> bool {
    match crate::proxy_autostart::stop_proxy_on_port(port) {
        Some(pid) => {
            crate::outln!("Proxy on port {port} stopped (PID {pid}).");
            true
        }
        None => false,
    }
}

/// Prints the proxy's live upstreams (from `/status`) and warns when one
/// differs from what config.toml resolves to — the proxy keeps its last good
/// value when an edit fails validation.
fn print_live_upstreams_and_drift(v: &serde_json::Value, cfg: &crate::core::config::Config) {
    let Some(up) = v.get("upstreams").and_then(|u| u.as_object()) else {
        return;
    };
    let configured = cfg.proxy.resolve_all();
    crate::outln!("  Upstreams (live):");
    let mut notes = Vec::new();
    for (label, key, configured_val) in [
        ("Anthropic", "anthropic", &configured.anthropic),
        ("OpenAI", "openai", &configured.openai),
    ] {
        let live = up.get(key).and_then(|x| x.as_str()).unwrap_or("?");
        crate::outln!("    {label:<10} {live}");
        if live != "?" && crate::core::config::upstream_drifted(configured_val, live) {
            notes.push(format!(
                "  \x1b[33m⚠ {label}: proxy serves {live} but config.toml resolves to {configured_val}.\x1b[0m\n  \
                   \x1b[33m  Check proxy.upstreams.{key}, then: sobre proxy restart\x1b[0m",
            ));
        }
    }
    for note in notes {
        crate::outln!();
        crate::outln!("{note}");
    }
}

pub(crate) fn cmd_proxy(rest: &[String]) {
    {
        // `--help` anywhere must never execute the verb.
        if wants_help(rest) {
            crate::outln!("{USAGE}");
            crate::outln!();
            crate::outln!("Commands:");
            crate::outln!(
                "  start     Run the compression proxy (foreground; -d/--detach for background; --autostart installs a service)"
            );
            crate::outln!("  stop      Unload the autostart unit and stop the proxy process");
            crate::outln!(
                "  restart   Reload the autostart unit, or restart the proxy process (re-reads config.toml)"
            );
            crate::outln!("  status    Show proxy config, process and live upstreams");
            crate::outln!(
                "  enable    Enable the proxy: config flag, autostart service, agent wiring (--force rewires agents that point elsewhere)"
            );
            crate::outln!("  disable   Disable the proxy and restore the original endpoint");
            crate::outln!("  cleanup   Remove stale proxy URLs from AI tool configs");
            crate::outln!("  token     Print the current proxy Bearer token (for HTTP clients)");
            return;
        }
        let Some(sub) = rest.first().map(String::as_str) else {
            crate::errln!("{USAGE}");
            std::process::exit(2);
        };
        match sub {
            "start" => {
                let port = port_flag(rest).unwrap_or_else(crate::proxy_setup::default_port);
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
                crate::proxy_setup::remember_started_port(port);
                if let Err(e) = crate::cli::dispatch::run_async(crate::proxy::start_proxy(port)) {
                    tracing::error!("Proxy error: {e}");
                    std::process::exit(1);
                }
            }
            "stop" => {
                let port = proxy_port(rest);
                // Unload the unit first, or launchd/systemd would start the
                // proxy again as soon as its process exits.
                let managed = crate::proxy_autostart::is_loaded();
                crate::proxy_autostart::stop();
                if managed {
                    crate::outln!("Autostart unit unloaded; the proxy is stopped.");
                } else if !stop_proxy_process(port) {
                    crate::outln!("No proxy running on port {port}.");
                }
            }
            "restart" => {
                let port = proxy_port(rest);
                if crate::proxy_autostart::is_installed() {
                    // Managed service (LaunchAgent / systemd): a clean bootout +
                    // bootstrap restarts the proxy so it re-reads config.toml. It
                    // makes config.toml the single source of truth for the
                    // long-lived proxy.
                    crate::proxy_autostart::stop();
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    if crate::proxy_autostart::start_on_port(port) {
                        crate::outln!("\x1b[32m✓\x1b[0m Proxy restarted (managed service).");
                        crate::outln!("  Verify active upstreams: sobre proxy status");
                    } else {
                        crate::errln!(
                            "\x1b[31m✗\x1b[0m Managed proxy restart failed; port {port} was not transferred."
                        );
                        std::process::exit(1);
                    }
                } else if stop_proxy_process(port) {
                    start_detached(port);
                } else {
                    crate::outln!("No proxy running on port {port}.");
                    crate::outln!("  Start it: sobre proxy start --port {port}");
                }
            }
            "status" => {
                let port = proxy_port(rest);
                let cfg = crate::core::config::Config::load();
                crate::outln!("sobre proxy:");
                match cfg.proxy.enabled {
                    Some(true) => crate::outln!("  Config:  enabled"),
                    Some(false) => crate::outln!("  Config:  disabled"),
                    None => crate::outln!("  Config:  undecided (not yet configured)"),
                }
                crate::outln!("  Port:    {port}");
                // Liveness comes from the *public* /health endpoint so a running
                // proxy is never misreported as down — even mid-upgrade when the
                // managed proxy still holds an old session token. The live
                // upstreams come from the authenticated /status; if that 401s
                // while /health is up, we still report it as running and point at
                // `proxy restart`. Usage and savings are in `sobre gain`.
                let alive = ureq::get(&format!("http://127.0.0.1:{port}/health"))
                    .call()
                    .is_ok();
                if alive {
                    crate::outln!("  Process: running");
                    let token =
                        crate::core::session_token::resolve_proxy_token("SOBRE_PROXY_TOKEN");
                    let status = ureq::get(&format!("http://127.0.0.1:{port}/status"))
                        .header("Authorization", &format!("Bearer {token}"))
                        .call();
                    if let Ok(resp) = status {
                        let body = resp.into_body().read_to_string().unwrap_or_default();
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                            print_live_upstreams_and_drift(&v, &cfg);
                        }
                    } else {
                        crate::outln!(
                            "  \x1b[33m⚠ Live details unavailable: the running proxy rejects this\x1b[0m"
                        );
                        crate::outln!(
                            "  \x1b[33m  shell's session token. Re-sync it: sobre proxy restart\x1b[0m"
                        );
                    }
                } else {
                    crate::outln!("  Process: not running");
                }
                if cfg.proxy.enabled == Some(false) || cfg.proxy.enabled.is_none() {
                    crate::outln!();
                    crate::outln!("  Enable: sobre proxy enable");

                    let home = dirs::home_dir().unwrap_or_default();
                    let stale = crate::proxy_setup::stale_proxy_agents(&home);
                    if !stale.is_empty() {
                        crate::outln!();
                        crate::outln!(
                            "  \x1b[33m⚠ WARNING: {} still point(s) to the local proxy,\x1b[0m",
                            stale.join(", ")
                        );
                        crate::outln!(
                            "  \x1b[33m  but proxy is not enabled, so those requests fail.\x1b[0m"
                        );
                        crate::outln!("  Fix:  sobre proxy cleanup   (remove stale URL)");
                        crate::outln!("        sobre proxy enable    (enable the proxy)");
                    }
                }
            }
            "enable" => {
                let force = rest.iter().any(|a| a == "--force");
                if let Err(e) =
                    crate::core::config::Config::update_global(|c| c.proxy.enabled = Some(true))
                {
                    tracing::warn!("could not persist proxy.enabled: {e}");
                }

                let port = crate::proxy_setup::default_port();
                if crate::proxy_autostart::is_supported() {
                    if !crate::proxy_autostart::install(port, false) {
                        crate::errln!(
                            "\x1b[31m✗\x1b[0m Proxy enable failed; managed service did not acquire port {port}."
                        );
                        std::process::exit(1);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                } else {
                    // No service manager integration here (Windows): the
                    // agents are still wired, and the proxy is started by hand.
                    crate::outln!(
                        "  No autostart on this platform: start the proxy with `sobre proxy start -d --port {port}`."
                    );
                }

                let home = dirs::home_dir().unwrap_or_default();
                crate::proxy_setup::install_proxy_env_unchecked(&home, port, false, force);
                crate::outln!(
                    "\x1b[32m✓\x1b[0m Proxy enabled on port {port}. LLM requests will be compressed."
                );
            }
            "disable" => {
                if let Err(e) =
                    crate::core::config::Config::update_global(|c| c.proxy.enabled = Some(false))
                {
                    tracing::warn!("could not persist proxy.enabled: {e}");
                }

                crate::proxy_autostart::uninstall(false);
                let home = dirs::home_dir().unwrap_or_default();
                crate::proxy_setup::uninstall_proxy_env(&home, false);

                crate::outln!("\x1b[32m✓\x1b[0m Proxy disabled. Original endpoint restored.");
                crate::outln!("  Re-enable anytime: sobre proxy enable");
            }
            "cleanup" => {
                let home = dirs::home_dir().unwrap_or_default();
                let removed = crate::proxy_setup::cleanup_stale_proxy_env(&home);
                if removed > 0 {
                    crate::outln!("\x1b[32m✓\x1b[0m Cleaned up {removed} stale proxy URL(s).");
                    crate::outln!("  Restart your AI tool for changes to take effect.");
                } else {
                    crate::outln!("  No stale proxy URLs found. Nothing to clean up.");
                }
            }
            "token" => {
                let token = crate::core::session_token::resolve_proxy_token("SOBRE_PROXY_TOKEN");
                let quiet = rest.iter().any(|a| a == "--quiet" || a == "-q");
                if quiet {
                    print!("{token}");
                } else {
                    crate::outln!("{token}");
                }
            }
            other => {
                crate::errln!("Unknown proxy command '{other}'.\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    /// Fork a child process running `sobre proxy start --port=N` with
    /// stdout/stderr redirected to a log file, then exit the parent immediately.
    /// The child's PID is written to the proxy PID file so `sobre proxy stop`
    /// can find and kill it.
    fn start_detached(port: u16) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "sobre".into());
        // The log lives in sobre's state dir, never a shared temporary one.
        let log_dir = crate::core::paths::state_dir().unwrap_or_else(|e| {
            crate::errln!("Cannot resolve sobre's state directory for the proxy log: {e}");
            std::process::exit(1);
        });
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("proxy.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap_or_else(|e| {
                crate::errln!("Cannot open {}: {e}", log_path.display());
                std::process::exit(1);
            });
        let stderr_file = log_file.try_clone().unwrap_or_else(|e| {
            crate::errln!("Cannot clone log fd: {e}");
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
                crate::proxy_setup::remember_started_port(port);
                crate::outln!(
                    "\x1b[32m✓\x1b[0m Proxy started in background (PID {}, port {port})",
                    child.id()
                );
                crate::outln!("  Logs: {}", log_path.display());
                crate::outln!("  Stop: sobre proxy stop");
            }
            Err(e) => {
                crate::errln!("Failed to start detached proxy: {e}");
                std::process::exit(1);
            }
        }
    }
}
