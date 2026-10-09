//! Shared proxy-setup constants and helpers.

pub(crate) const DEFAULT_PROXY_PORT: u16 = 4444;

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

/// Whether something accepts a TCP connection on `port` on localhost within
/// 200 ms.
pub(crate) fn is_proxy_reachable(port: u16) -> bool {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
}

pub fn default_port() -> u16 {
    let cfg = crate::core::config::Config::load();
    if let Some(port) = cfg.proxy.port {
        return port;
    }
    uid_based_port()
}

/// The port of this user's proxy when `proxy.port` is unset: 4444 + uid
/// mod 1000 on Unix, so two people on one machine get different ports
/// (uid 1000 → 4444, macOS's first user 501 → 4945); 4444 on Windows.
fn uid_based_port() -> u16 {
    #[cfg(unix)]
    {
        // SAFETY: `getuid` takes no arguments, always succeeds, and only reads
        // the calling process's real UID — no preconditions, no UB.
        let uid = unsafe { libc::getuid() };
        port_for_uid(uid)
    }
    #[cfg(not(unix))]
    {
        DEFAULT_PROXY_PORT
    }
}

#[cfg(unix)]
fn port_for_uid(uid: u32) -> u16 {
    DEFAULT_PROXY_PORT + u16::try_from(uid % 1000).unwrap_or_default()
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn every_uid_gets_its_own_port_modulo_1000() {
        assert_eq!(super::port_for_uid(0), 4444);
        assert_eq!(super::port_for_uid(501), 4945);
        assert_eq!(super::port_for_uid(1000), 4444);
        assert_eq!(super::port_for_uid(1001), 4445);
        // No truncation to 16 bits: 65536 + 1 is not uid 1.
        assert_eq!(super::port_for_uid(65_537), 4444 + 537);
    }
}
