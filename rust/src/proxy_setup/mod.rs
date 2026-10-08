//! Point installed agents at the proxy, each through its own configuration:
//! Claude Code's `settings.json`, Codex's `config.toml` and OpenCode's
//! `opencode.json`. Nothing is written into shell rc files.
//!
//! Submodules hold per-client setup. This facade keeps the public API stable
//! as `crate::proxy_setup::*`.

mod claude;
mod codex;
mod opencode;
mod util;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::path::Path;

use claude::{install_claude_env, install_claude_env_inner, uninstall_claude_env};
use codex::{codex_config_has_local_proxy_entry, strip_codex_proxy_entries, uninstall_codex_env};
use opencode::{has_proxy_base_url, install_opencode_env, uninstall_opencode_env};

pub use claude::anthropic_api_key_available;
pub(crate) use codex::install_codex_env;
pub use util::{default_port, is_local_sobre_url, proxy_timeout};

pub fn install_proxy_env(home: &Path, port: u16, quiet: bool) {
    let cfg = crate::core::config::Config::load();
    if cfg.proxy.enabled != Some(true) {
        if !quiet {
            println!("  Proxy env skipped (not enabled in config)");
        }
        return;
    }
    install_claude_env(home, port, quiet);
    install_codex_env(home, port, quiet);
    install_opencode_env(home, port, quiet);
}

/// Install proxy env without config guard (used by `sobre proxy enable` which has already set the flag).
/// `force_endpoint`: if true, overrides even non-local custom endpoints.
pub fn install_proxy_env_unchecked(home: &Path, port: u16, quiet: bool, force_endpoint: bool) {
    if force_endpoint {
        install_claude_env_inner(home, port, quiet, true);
    } else {
        install_claude_env(home, port, quiet);
    }
    install_codex_env(home, port, quiet);
    install_opencode_env(home, port, quiet);
}

pub fn preview_proxy_cleanup(home: &Path) {
    let settings_dir = crate::core::editor_registry::claude_state_dir(home);
    let settings_path = settings_dir.join("settings.json");
    if std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|content| crate::core::jsonc::parse_jsonc(&content).ok())
        .is_some_and(|doc| {
            doc.get("env")
                .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
                .and_then(|v| v.as_str())
                .is_some_and(is_local_sobre_url)
        })
    {
        let cfg = crate::core::config::Config::load();
        if let Some(ref upstream) = cfg.proxy.upstreams.get("anthropic").cloned() {
            println!("  Would restore ANTHROPIC_BASE_URL → {upstream} in Claude Code settings");
        } else {
            println!("  Would remove ANTHROPIC_BASE_URL from Claude Code settings");
        }
    }

    let codex_path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| home.join(".codex/config.toml"));
    if let Ok(content) = std::fs::read_to_string(codex_path)
        && codex_config_has_local_proxy_entry(&content)
    {
        println!("  Would remove Codex proxy URL from config.toml");
    }

    if has_proxy_base_url(home) {
        println!("  Would remove the proxy base URLs from OpenCode's opencode.json");
    }
}

/// Removes stale proxy URLs from Claude Code / Codex / OpenCode settings when the proxy is not enabled.
/// Returns the number of stale URLs cleaned up.
pub fn cleanup_stale_proxy_env(home: &Path) -> usize {
    let cfg = crate::core::config::Config::load();
    if cfg.proxy.enabled == Some(true) {
        return 0;
    }

    let mut cleaned = 0;

    let settings_dir = crate::core::editor_registry::claude_state_dir(home);
    let settings_path = settings_dir.join("settings.json");
    if let Ok(content) = std::fs::read_to_string(&settings_path)
        && let Ok(mut doc) = crate::core::jsonc::parse_jsonc(&content)
        && let Some(base_url) = doc
            .get("env")
            .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
            .and_then(|v| v.as_str())
            .map(String::from)
        && is_local_sobre_url(&base_url)
        && let Some(env_obj) = doc.get_mut("env").and_then(|e| e.as_object_mut())
    {
        if let Some(ref upstream) = cfg.proxy.upstreams.get("anthropic").cloned() {
            env_obj.insert(
                "ANTHROPIC_BASE_URL".to_string(),
                serde_json::Value::String(upstream.clone()),
            );
            println!("  ✓ Restored ANTHROPIC_BASE_URL → {upstream} in Claude Code settings");
        } else {
            env_obj.remove("ANTHROPIC_BASE_URL");
            if env_obj.is_empty() {
                doc.as_object_mut().map(|o| o.remove("env"));
            }
            println!("  ✓ Removed stale ANTHROPIC_BASE_URL from Claude Code settings");
        }
        let _ = crate::config_io::write_json_cleanup(&settings_path, &content, &doc);
        cleaned += 1;
    }

    let codex_path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| home.join(".codex/config.toml"));
    if let Ok(content) = std::fs::read_to_string(&codex_path)
        && codex_config_has_local_proxy_entry(&content)
    {
        let filtered = strip_codex_proxy_entries(&content);
        let _ = std::fs::write(&codex_path, &filtered);
        println!("  ✓ Removed stale Codex proxy URL from config.toml");
        cleaned += 1;
    }

    if uninstall_opencode_env(home, false) {
        cleaned += 1;
    }

    cleaned
}

/// The agents whose configuration still points at the local proxy although
/// the proxy is not enabled: what [`cleanup_stale_proxy_env`] repairs.
pub fn stale_proxy_agents(home: &Path) -> Vec<&'static str> {
    let cfg = crate::core::config::Config::load();
    if cfg.proxy.enabled == Some(true) {
        return Vec::new();
    }

    let mut stale = Vec::new();
    let settings_path = crate::core::editor_registry::claude_state_dir(home).join("settings.json");
    if std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|content| crate::core::jsonc::parse_jsonc(&content).ok())
        .is_some_and(|doc| {
            doc.get("env")
                .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
                .and_then(|v| v.as_str())
                .is_some_and(is_local_sobre_url)
        })
    {
        stale.push("Claude Code");
    }

    let codex_path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| home.join(".codex/config.toml"));
    if std::fs::read_to_string(codex_path)
        .is_ok_and(|content| codex_config_has_local_proxy_entry(&content))
    {
        stale.push("Codex");
    }

    if has_proxy_base_url(home) {
        stale.push("OpenCode");
    }
    stale
}

pub fn uninstall_proxy_env(home: &Path, quiet: bool) {
    uninstall_claude_env(home, quiet);
    uninstall_codex_env(home, quiet);
    uninstall_opencode_env(home, quiet);
}
