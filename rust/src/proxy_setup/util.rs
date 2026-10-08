//! Shared proxy-setup constants and helpers.

pub(crate) const DEFAULT_PROXY_PORT: u16 = 4444;

pub fn is_local_sobre_url(url: &str) -> bool {
    url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:")
}

/// Proxy reachability timeout (200 ms).
pub fn proxy_timeout() -> std::time::Duration {
    std::time::Duration::from_millis(200)
}

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
