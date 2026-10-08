//! LLM proxy plane: liveness, auth, upstream config, env drift, the
//! Claude-subscription conflict.

#[allow(clippy::wildcard_imports)]
use crate::doctor::common::*;
use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST, YELLOW};

pub(crate) fn proxy_health_outcome() -> Outcome {
    use crate::core::config::Config;

    let cfg = Config::load();
    let port = crate::proxy_setup::default_port();

    match cfg.proxy.enabled {
        Some(true) => {
            let reachable = {
                use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
                let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
                TcpStream::connect_timeout(&addr, crate::proxy_setup::proxy_timeout()).is_ok()
            };
            // Autostart has no backend on Windows/other platforms, so a missing
            // autostart must never be a hard failure there (#416).
            let supported = crate::proxy_autostart::is_supported();

            if reachable {
                // Up now — verify the HTTP/auth layer regardless of autostart state.
                if !proxy_auth_probe(port) {
                    return Outcome {
                        ok: false,
                        line: format!(
                            "{BOLD}Proxy{RST}  {YELLOW}running on port {port} but auth probe failed{RST}  {YELLOW}fix: lean-ctx proxy restart{RST}"
                        ),
                    };
                }
                if supported && !crate::proxy_autostart::is_installed() {
                    // Running, but it won't survive a reboot without autostart.
                    Outcome {
                        ok: true,
                        line: format!(
                            "{BOLD}Proxy{RST}  {GREEN}running on port {port}{RST}  {YELLOW}autostart not installed — persist: lean-ctx proxy enable{RST}"
                        ),
                    }
                } else {
                    Outcome {
                        ok: true,
                        line: format!(
                            "{BOLD}Proxy{RST}  {GREEN}enabled, running on port {port}{RST}"
                        ),
                    }
                }
            } else if supported {
                Outcome {
                    ok: false,
                    line: format!(
                        "{BOLD}Proxy{RST}  {RED}enabled but not reachable on port {port}{RST}  {YELLOW}fix: lean-ctx proxy start{RST}"
                    ),
                }
            } else {
                // Windows/other: no autostart backend, so a stopped proxy is a
                // setup note (start it manually), not a doctor failure (#416).
                Outcome {
                    ok: true,
                    line: format!(
                        "{BOLD}Proxy{RST}  {YELLOW}enabled but not running{RST}  {DIM}autostart unavailable on this platform — start: lean-ctx proxy start{RST}"
                    ),
                }
            }
        }
        Some(false) => Outcome {
            ok: true,
            line: format!(
                "{BOLD}Proxy{RST}  {DIM}disabled (optional feature){RST}  {DIM}enable: lean-ctx proxy enable{RST}"
            ),
        },
        None => Outcome {
            ok: true,
            line: format!(
                "{BOLD}Proxy{RST}  {DIM}not configured{RST}  {DIM}enable: lean-ctx proxy enable{RST}"
            ),
        },
    }
}
/// Detects agent configurations (Claude Code, Codex, OpenCode) that still
/// point at the local proxy while the proxy is not enabled. Returns `None`
/// when there are none.
pub(crate) fn stale_proxy_env_outcome() -> Option<Outcome> {
    use crate::core::config::Config;

    let home = dirs::home_dir()?;
    let stale = crate::proxy_setup::stale_proxy_agents(&home);
    if stale.is_empty() {
        return None;
    }
    let state = if Config::load().proxy.enabled == Some(false) {
        "disabled"
    } else {
        "not configured"
    };
    let agents = stale.join(", ");

    Some(Outcome {
        ok: false,
        line: format!(
            "{BOLD}Proxy env{RST}  {RED}{agents} → local proxy, but proxy is {state}{RST}\n\
             {DIM}         Their API requests go to a proxy that is {state} and fail. Fix:{RST}\n\
             {YELLOW}           lean-ctx proxy cleanup    {DIM}(remove stale URL){RST}\n\
             {YELLOW}           lean-ctx proxy enable     {DIM}(enable the proxy){RST}"
        ),
    })
}
/// Detects the Claude Pro/Max subscription + proxy conflict: the proxy is enabled and
/// Claude Code's `ANTHROPIC_BASE_URL` points at the local proxy, but no Anthropic API
/// key is available. A subscription OAuth token only authenticates against
/// `api.anthropic.com`, so routing it through the proxy causes a login loop / 401.
/// Returns `None` when not applicable, `Some(Outcome)` when the conflict is present.
pub(crate) fn proxy_subscription_conflict_outcome() -> Option<Outcome> {
    use crate::core::config::Config;

    let home = dirs::home_dir()?;
    let cfg = Config::load();

    // Only relevant when the proxy is actively enabled.
    if cfg.proxy.enabled != Some(true) {
        return None;
    }

    let settings_path = crate::core::editor_registry::claude_state_dir(&home).join("settings.json");
    let content = std::fs::read_to_string(&settings_path).ok()?;
    let doc: serde_json::Value = crate::core::jsonc::parse_jsonc(&content).ok()?;

    let base_url = doc
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // No local redirect → nothing to warn about.
    if !crate::proxy_setup::is_local_lean_ctx_url(base_url) {
        return None;
    }

    // API key present → the proxy can forward it, redirect is fine.
    if crate::proxy_setup::anthropic_api_key_available(&home) {
        return None;
    }

    Some(Outcome {
        ok: false,
        line: format!(
            "{BOLD}Claude auth{RST}  {RED}ANTHROPIC_BASE_URL → proxy but no ANTHROPIC_API_KEY (Pro/Max subscription){RST}\n\
             {DIM}         A subscription token only authenticates against api.anthropic.com; routing it{RST}\n\
             {DIM}         through the proxy causes a login loop / 401. Fix one of:{RST}\n\
             {YELLOW}           lean-ctx proxy disable     {DIM}(keep your subscription; use ctx_* MCP tools for savings){RST}\n\
             {YELLOW}           export ANTHROPIC_API_KEY=…  {DIM}then: lean-ctx proxy enable  (pay-as-you-go via proxy){RST}"
        ),
    })
}
pub(crate) fn proxy_upstream_outcome() -> Outcome {
    let cfg = crate::core::config::Config::load();
    upstream_outcome(&cfg.proxy, crate::proxy_setup::default_port())
}

/// The upstream check for `proxy`, served on `proxy_port`. Only an upstream
/// that is the proxy's own address fails; a loopback URL on another port is a
/// local model server, such as Ollama on `http://127.0.0.1:11434`.
pub(super) fn upstream_outcome(
    proxy: &crate::core::config::ProxyConfig,
    proxy_port: u16,
) -> Outcome {
    use crate::core::config::{ProxyProvider, is_local_proxy_url};

    let checks = [
        (
            "Anthropic",
            "proxy.upstreams.anthropic",
            proxy.resolve_upstream(ProxyProvider::Anthropic),
        ),
        (
            "OpenAI",
            "proxy.upstreams.openai",
            proxy.resolve_upstream(ProxyProvider::OpenAi),
        ),
    ];

    let mut custom = Vec::new();
    let mut plaintext = Vec::new();
    for (label, key, resolved) in &checks {
        let loopback = is_local_proxy_url(resolved);
        let port = resolved
            .rsplit_once(':')
            .and_then(|(_, rest)| rest.split('/').next()?.parse::<u16>().ok());
        if loopback && port == Some(proxy_port) {
            return Outcome {
                ok: false,
                line: format!(
                    "{BOLD}Proxy upstream{RST}  {RED}{label} upstream points back to the proxy itself{RST}  {YELLOW}run: lean-ctx config set {key} <url>{RST}"
                ),
            };
        }
        if !resolved.starts_with("http://") && !resolved.starts_with("https://") {
            return Outcome {
                ok: false,
                line: format!(
                    "{BOLD}Proxy upstream{RST}  {RED}invalid {label} upstream{RST}  {YELLOW}set {key} to an http(s) URL{RST}"
                ),
            };
        }
        let is_default = matches!(
            *label,
            "Anthropic" if resolved == "https://api.anthropic.com"
        ) || matches!(
            *label,
            "OpenAI" if resolved == "https://api.openai.com"
        );
        if !is_default {
            custom.push(format!("{label}={resolved}"));
            // Plaintext to another machine: valid config, but worth a
            // standing security reminder. Loopback never leaves the machine.
            if resolved.starts_with("http://") && !loopback {
                plaintext.push(*label);
            }
        }
    }

    if custom.is_empty() {
        Outcome {
            ok: true,
            line: format!("{BOLD}Proxy upstream{RST}  {GREEN}provider defaults{RST}"),
        }
    } else {
        let mut line = format!(
            "{BOLD}Proxy upstream{RST}  {GREEN}custom: {}{RST}",
            custom.join(", ")
        );
        if !plaintext.is_empty() {
            line.push_str(&format!(
                "  {YELLOW}⚠ plaintext HTTP ({}) — trusted local network only{RST}",
                plaintext.join(", ")
            ));
        }
        Outcome { ok: true, line }
    }
}
/// #449 drift check: warns when the running proxy forwards to a different
/// upstream than config.toml resolves to (an edit that failed validation).
/// Returns `None` when the proxy is down or in sync, so the board stays quiet
/// unless there is something actionable.
pub(crate) fn proxy_upstream_drift_outcome() -> Option<Outcome> {
    use crate::core::config::{Config, upstream_drifted};

    let cfg = Config::load();
    if cfg.proxy.enabled != Some(true) {
        return None;
    }
    let port = crate::proxy_setup::default_port();
    let (live_anthropic, live_openai) = proxy_live_upstreams(port)?;
    let configured = cfg.proxy.resolve_all();

    let drifted: Vec<String> = [
        ("Anthropic", &configured.anthropic, &live_anthropic),
        ("OpenAI", &configured.openai, &live_openai),
    ]
    .into_iter()
    .filter(|(_, configured, live)| upstream_drifted(configured, live))
    .map(|(label, configured, live)| format!("{label} live {live} ≠ config {configured}"))
    .collect();

    if drifted.is_empty() {
        return None;
    }
    Some(Outcome {
        ok: false,
        line: format!(
            "{BOLD}Proxy upstream drift{RST}  {YELLOW}{} — check the upstream in config.toml, then: lean-ctx proxy restart{RST}",
            drifted.join("; ")
        ),
    })
}
