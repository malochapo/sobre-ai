//! Claude Code settings proxy env wiring.

use std::path::Path;

use super::util::is_proxy_reachable;
use super::wiring;

/// The wiring slot of Claude Code's `ANTHROPIC_BASE_URL`.
const SLOT: &str = "claude";

/// Returns true when an Anthropic **API key** is available for the proxy to forward
/// upstream.
///
/// The proxy never injects credentials (it forwards the client's own), so it
/// can only help Claude Code when the
/// user runs in API-key (pay-as-you-go) mode. A Claude **Pro/Max subscription**
/// authenticates via OAuth directly against `api.anthropic.com`; that token is rejected
/// by any custom `ANTHROPIC_BASE_URL`, so redirecting subscription traffic through the
/// proxy only breaks auth (login loop / 401). When this returns `false`, callers must
/// NOT point Claude Code at the proxy.
pub fn anthropic_api_key_available(home: &Path) -> bool {
    // 1) Process environment — covers shells and Claude Code launched from them.
    for var in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
        if std::env::var(var).is_ok_and(|v| !v.trim().is_empty()) {
            return true;
        }
    }

    // 2) Claude Code settings.json — an explicit key, an auth token, or a dynamic
    //    key helper all indicate API-key mode.
    let settings_path = crate::core::editor_registry::claude_state_dir(home).join("settings.json");
    let Ok(content) = std::fs::read_to_string(&settings_path) else {
        return false;
    };
    let Ok(doc) = crate::core::jsonc::parse_jsonc(&content) else {
        return false;
    };

    if doc
        .get("apiKeyHelper")
        .and_then(|v| v.as_str())
        .is_some_and(|v| !v.trim().is_empty())
    {
        return true;
    }

    let env = doc.get("env");
    ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .iter()
        .any(|key| {
            env.and_then(|e| e.get(*key))
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.trim().is_empty())
        })
}

/// Explains why Claude Code was left pointing at `api.anthropic.com` instead of the
/// proxy: a Pro/Max subscription (OAuth) cannot authenticate through a custom base URL.
fn warn_claude_subscription_skip() {
    eprintln!("  \u{26a0} Claude Code: no ANTHROPIC_API_KEY detected (Pro/Max subscription?).");
    eprintln!("    The proxy forwards your credential upstream but never injects one, and a");
    eprintln!("    subscription token only authenticates against api.anthropic.com directly.");
    eprintln!("    Leaving ANTHROPIC_BASE_URL untouched so Claude Code keeps working.");
    eprintln!("    Savings on a subscription: use the sobre MCP tools (sobre_read /");
    eprintln!("    sobre_search / sobre_shell). Pay-as-you-go? Set ANTHROPIC_API_KEY, then run:");
    eprintln!("      sobre proxy enable");
}

/// Whether Claude Code's `ANTHROPIC_BASE_URL` is the one sobre wrote.
pub(crate) fn claude_points_at_proxy(home: &Path) -> bool {
    let settings_path = crate::core::editor_registry::claude_state_dir(home).join("settings.json");
    std::fs::read_to_string(settings_path)
        .ok()
        .and_then(|content| crate::core::jsonc::parse_jsonc(&content).ok())
        .is_some_and(|doc| {
            doc.get("env")
                .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
                .and_then(|v| v.as_str())
                .is_some_and(|url| wiring::is_ours(SLOT, url))
        })
}

/// Undoes sobre's `ANTHROPIC_BASE_URL` in `doc`: the value the proxy
/// replaced comes back, or the key goes (and an `env` it leaves empty).
/// `None`, and nothing changed, when the URL there is not sobre's: an
/// endpoint the user set stays as it is.
fn undo_base_url(doc: &mut serde_json::Value) -> Option<String> {
    let env_obj = doc.get_mut("env")?.as_object_mut()?;
    let current = env_obj.get("ANTHROPIC_BASE_URL")?.as_str()?;
    if !wiring::is_ours(SLOT, current) {
        return None;
    }
    if let Some(previous) = wiring::forget(SLOT).and_then(|r| r.displaced) {
        let message = format!("✓ Restored ANTHROPIC_BASE_URL → {previous} in Claude Code settings");
        env_obj.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            serde_json::Value::String(previous),
        );
        return Some(message);
    }
    env_obj.remove("ANTHROPIC_BASE_URL");
    if env_obj.is_empty() {
        doc.as_object_mut().map(|o| o.remove("env"));
    }
    Some("✓ Removed ANTHROPIC_BASE_URL from Claude Code settings".to_string())
}

/// Takes Claude Code off the proxy when sobre put it there. Returns whether
/// it changed anything.
pub(crate) fn uninstall_claude_env(home: &Path, quiet: bool) -> bool {
    let settings_dir = crate::core::editor_registry::claude_state_dir(home);
    let settings_path = settings_dir.join("settings.json");
    let existing = match std::fs::read_to_string(&settings_path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return false,
    };
    let Ok(mut doc) = crate::core::jsonc::parse_jsonc(&existing) else {
        return false;
    };
    let Some(message) = undo_base_url(&mut doc) else {
        return false;
    };
    let _ = crate::config_io::write_json_cleanup(&settings_path, &existing, &doc);
    if !quiet {
        println!("  {message}");
    }
    true
}

pub(crate) fn install_claude_env(home: &Path, port: u16, quiet: bool) {
    install_claude_env_inner(home, port, quiet, false);
}

pub(crate) fn install_claude_env_inner(home: &Path, port: u16, quiet: bool, force: bool) {
    use crate::core::config::normalize_url_opt;

    let base = format!("http://127.0.0.1:{port}");

    let settings_dir = crate::core::editor_registry::claude_state_dir(home);
    let settings_path = settings_dir.join("settings.json");
    let existing = std::fs::read_to_string(&settings_path).unwrap_or_default();
    let mut doc: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        match crate::core::jsonc::parse_jsonc(&existing) {
            Ok(v) => v,
            Err(_) => return,
        }
    };

    let current_url = doc
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // SUBSCRIPTION GUARD: the proxy never injects credentials, so redirecting Claude
    // Code only works in API-key mode. A Claude Pro/Max subscription (OAuth) is rejected
    // by a custom ANTHROPIC_BASE_URL → login loop / 401. When no API key is detectable we
    // must not point Claude Code at the proxy. `--force` overrides for power users whose
    // key lives somewhere we cannot probe (e.g. a keychain or apiKeyHelper we missed).
    if !force && !anthropic_api_key_available(home) {
        // Undo a redirect sobre made earlier, so Claude Code reaches Anthropic again.
        if undo_base_url(&mut doc).is_some() {
            let _ = crate::config_io::write_json_cleanup(&settings_path, &existing, &doc);
        }
        if !quiet {
            warn_claude_subscription_skip();
        }
        return;
    }

    if current_url == base {
        wiring::record(SLOT, &base, None);
        if !quiet {
            println!("  Claude Code proxy env already configured");
        }
        return;
    }

    // An endpoint the user set — remote or local (LiteLLM, Ollama) — is never
    // replaced unless --force; with --force the proxy forwards to it, and
    // undoing the wiring puts it back.
    let mut custom_upstream = None;
    if let Some(upstream) = normalize_url_opt(&current_url)
        && !wiring::is_ours(SLOT, &current_url)
    {
        if !force {
            if !quiet {
                eprintln!("  \u{26a0} Claude Code already points at a custom endpoint: {upstream}");
                eprintln!("    Left as it is. `sobre proxy enable --force` routes Claude Code");
                eprintln!("    through the proxy, which then forwards to that endpoint.");
            }
            return;
        }
        custom_upstream = Some(upstream);
    }

    if !is_proxy_reachable(port) {
        if !quiet {
            println!("  Skipping Claude Code proxy env (proxy not running on port {port})");
        }
        return;
    }

    if let Some(ref upstream) = custom_upstream {
        keep_custom_endpoint_as_upstream(upstream, quiet);
    }
    wiring::record(
        SLOT,
        &base,
        custom_upstream.is_some().then_some(current_url.as_str()),
    );

    if let Some(env_obj) = doc.as_object_mut().and_then(|o| {
        o.entry("env")
            .or_insert(serde_json::json!({}))
            .as_object_mut()
    }) {
        env_obj.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            serde_json::Value::String(base),
        );
    }

    let _ = crate::config_io::write_json_edit(&settings_path, &existing, &doc);
    if !quiet {
        println!("  Configured ANTHROPIC_BASE_URL in Claude Code settings");
    }
}

/// With `--force`, the proxy replaces the custom endpoint in Claude Code's
/// settings, so the proxy must forward there: the endpoint becomes
/// `proxy.upstreams.anthropic` in config.toml, unless the user already set one.
fn keep_custom_endpoint_as_upstream(upstream: &str, quiet: bool) {
    use crate::core::config::Config;

    let path =
        Config::path().map_or_else(|| "config.toml".to_string(), |p| p.display().to_string());
    if let Some(existing) = Config::load_global().proxy.upstreams.get("anthropic") {
        if !quiet && existing != upstream {
            println!(
                "  Claude Code pointed at {upstream}, but proxy.upstreams.anthropic in {path} \
                 is {existing}: the proxy forwards there."
            );
        }
        return;
    }
    match Config::update_global(|c| {
        c.proxy
            .upstreams
            .insert("anthropic".to_string(), upstream.to_string());
    }) {
        Ok(_) => {
            if !quiet {
                println!("  Saved Claude Code's endpoint {upstream} as proxy.upstreams.anthropic");
                println!("    in {path}: the proxy forwards Claude Code's requests there,");
                println!("    and `sobre proxy disable` or `sobre uninstall` puts it back.");
            }
        }
        Err(e) => eprintln!("  \u{26a0} Could not save {upstream} as the proxy's upstream: {e}"),
    }
}
