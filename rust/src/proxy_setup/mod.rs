//! Point installed agents at the proxy, each through its own configuration:
//! Claude Code's `settings.json`, Codex's `config.toml` and OpenCode's
//! `opencode.json`. Nothing is written into shell rc files. One submodule per
//! agent.

mod claude;
mod codex;
mod opencode;
mod util;
mod wiring;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::path::Path;

use claude::{install_claude_env, install_claude_env_inner, uninstall_claude_env};
use codex::{codex_points_at_proxy, uninstall_codex_env};
use opencode::{has_proxy_base_url, install_opencode_env, uninstall_opencode_env};

pub use claude::anthropic_api_key_available;
pub(crate) use claude::claude_points_at_proxy;
pub(crate) use codex::install_codex_env;
pub use util::default_port;
pub(crate) use util::{is_proxy_reachable, last_started_port, remember_started_port};

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

/// Removes stale proxy URLs from Claude Code / Codex / OpenCode settings when the proxy is not enabled.
/// Returns the number of stale URLs cleaned up.
pub fn cleanup_stale_proxy_env(home: &Path) -> usize {
    let cfg = crate::core::config::Config::load();
    if cfg.proxy.enabled == Some(true) {
        return 0;
    }
    usize::from(uninstall_claude_env(home, false))
        + usize::from(uninstall_codex_env(home, false))
        + usize::from(uninstall_opencode_env(home, false))
}

/// The agents whose configuration still points at the local proxy although
/// the proxy is not enabled: what [`cleanup_stale_proxy_env`] repairs.
pub fn stale_proxy_agents(home: &Path) -> Vec<&'static str> {
    let cfg = crate::core::config::Config::load();
    if cfg.proxy.enabled == Some(true) {
        return Vec::new();
    }
    let mut stale = Vec::new();
    if claude_points_at_proxy(home) {
        stale.push("Claude Code");
    }
    if codex_points_at_proxy(home) {
        stale.push("Codex");
    }
    if has_proxy_base_url(home) {
        stale.push("OpenCode");
    }
    stale
}

/// Takes every agent sobre wired off the proxy; returns how many it changed.
pub fn uninstall_proxy_env(home: &Path, quiet: bool) -> usize {
    usize::from(uninstall_claude_env(home, quiet))
        + usize::from(uninstall_codex_env(home, quiet))
        + usize::from(uninstall_opencode_env(home, quiet))
}
