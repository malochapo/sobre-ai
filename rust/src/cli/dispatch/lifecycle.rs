//! Proxy auto-spawn at MCP server startup.

use crate::core;

/// Whether this process, rather than a managed LaunchAgent/systemd unit, owns startup.
fn should_spawn_proxy_locally(managed_service_loaded: bool, already_running: bool) -> bool {
    !managed_service_loaded && !already_running
}

pub(super) fn spawn_proxy_if_needed() {
    let cfg = core::config::Config::load();
    if cfg.proxy.enabled != Some(true) {
        return;
    }

    let managed_service_loaded = crate::proxy_autostart::is_loaded();
    let port = crate::proxy_setup::default_port();
    let already_running = crate::proxy_setup::is_proxy_reachable(port);

    if !should_spawn_proxy_locally(managed_service_loaded, already_running) {
        if managed_service_loaded {
            tracing::debug!("managed proxy service owns startup on port {port}");
        } else if already_running {
            tracing::debug!("proxy already running on port {port}");
        }
        return;
    }

    let binary = core::portable_binary::resolve_portable_binary();

    let mut cmd = std::process::Command::new(&binary);
    cmd.args(["proxy", "start", &format!("--port={port}")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Detached spawn: on Windows the proxy must escape the MCP process's
    // console/Job or it dies when the AI client recycles the MCP server.
    match crate::ipc::process::spawn_detached(&mut cmd) {
        Ok(_) => tracing::info!("auto-started proxy on port {port}"),
        Err(e) => tracing::debug!("could not auto-start proxy: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::should_spawn_proxy_locally;

    #[test]
    fn managed_proxy_service_prevents_a_duplicate_local_spawn() {
        assert!(should_spawn_proxy_locally(false, false));
        assert!(!should_spawn_proxy_locally(true, false));
        assert!(!should_spawn_proxy_locally(false, true));
    }
}
