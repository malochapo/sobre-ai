//! Shared proxy-setup constants and helpers.

pub(crate) const DEFAULT_PROXY_PORT: u16 = 4444;

pub fn is_local_sobre_url(url: &str) -> bool {
    url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:")
}

/// Where the port of the last proxy started from the CLI is kept, so
/// `proxy status`, `stop`, `restart` and `uninstall` find a proxy started on
/// another port.
fn started_port_file() -> Option<std::path::PathBuf> {
    crate::core::paths::state_dir()
        .ok()
        .map(|d| d.join("proxy.port"))
}

/// Records `port` as the one the last proxy was started on.
pub(crate) fn remember_started_port(port: u16) {
    if let Some(path) = started_port_file() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, port.to_string());
    }
}

/// The port the last proxy was started on, if one was.
pub(crate) fn last_started_port() -> Option<u16> {
    started_port_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse().ok())
}

/// Proxy reachability timeout (200 ms).
pub fn proxy_timeout() -> std::time::Duration {
    std::time::Duration::from_millis(200)
}

/// Whether something accepts a TCP connection on `port` on localhost.
pub(crate) fn is_proxy_reachable(port: u16) -> bool {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpStream::connect_timeout(&addr, proxy_timeout()).is_ok()
}

pub fn default_port() -> u16 {
    let cfg = crate::core::config::Config::load();
    if let Some(port) = cfg.proxy.port {
        return port;
    }
    uid_based_port()
}

/// Derives a deterministic port from the user's UID to avoid collisions
/// on multi-user systems. uid 1000 → 4444, uid 1001 → 4445, etc.
/// System accounts (uid < 1000) and root always get the base port 4444.
pub(crate) fn uid_based_port() -> u16 {
    #[cfg(unix)]
    {
        // SAFETY: `getuid` takes no arguments, always succeeds, and only reads
        // the calling process's real UID — no preconditions, no UB.
        let uid = unsafe { libc::getuid() } as u16;
        let offset = uid.saturating_sub(1000) % 1000;
        DEFAULT_PROXY_PORT + offset
    }
    #[cfg(not(unix))]
    {
        DEFAULT_PROXY_PORT
    }
}
