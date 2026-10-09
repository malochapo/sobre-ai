#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
const PLIST_LABEL: &str = "com.sobre.proxy";
#[cfg(target_os = "linux")]
const SYSTEMD_SERVICE: &str = "sobre-proxy";

fn proxy_pid_from_health(body: &str) -> Option<u32> {
    let health: serde_json::Value = serde_json::from_str(body).ok()?;
    if health.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return None;
    }
    let pid = u32::try_from(health.get("pid")?.as_u64()?).ok()?;
    (pid > 0).then_some(pid)
}

fn health_identifies_sobre_proxy(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|health| health.get("service")?.as_str().map(str::to_owned))
        .is_some_and(|service| service == "sobre-proxy")
}

/// The PID of the sobre proxy answering `/health` on `port`, or `None` when
/// nothing answers there or the answer is not a sobre proxy's.
fn proxy_pid_on_port(port: u16) -> Option<u32> {
    let health_url = format!("http://127.0.0.1:{port}/health");
    let response = ureq::get(&health_url)
        .config()
        .timeout_global(Some(Duration::from_millis(500)))
        .build()
        .call()
        .ok()?;
    let body = response.into_body().read_to_string().ok()?;
    let pid = proxy_pid_from_health(&body)?;
    health_identifies_sobre_proxy(&body).then_some(pid)
}

/// Stops the sobre proxy answering on `port` (SIGTERM, then SIGKILL after
/// two seconds) and returns its PID. `None` when no sobre proxy answers there:
/// a process is only ever signalled after it identified itself as sobre's
/// proxy, so nothing else on the machine is touched.
pub(crate) fn stop_proxy_on_port(port: u16) -> Option<u32> {
    let pid = proxy_pid_on_port(port)?;
    if pid == std::process::id() {
        return None;
    }
    let _ = crate::ipc::process::terminate_gracefully(pid);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && crate::ipc::process::is_alive(pid) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if crate::ipc::process::is_alive(pid) {
        let _ = crate::ipc::process::force_kill(pid);
    }
    Some(pid)
}

/// Stop a proxy already owning `port` before a managed service is started.
///
/// Returns `false` when the listener cannot be positively identified as a
/// sobre proxy or when it does not release the port. Managed startup must
/// fail closed in either case; otherwise launchd/systemd would enter a restart
/// loop on `EADDRINUSE`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn release_proxy_port(port: u16, quiet: bool) -> bool {
    if !crate::proxy_setup::is_proxy_reachable(port) {
        return true;
    }

    let Some(pid) = proxy_pid_on_port(port) else {
        if !quiet {
            eprintln!(
                "  Refusing managed proxy startup: port {port} is occupied by an unidentified service."
            );
        }
        return false;
    };
    if pid == std::process::id() {
        if !quiet {
            eprintln!("  Refusing to stop the current process while handing off port {port}.");
        }
        return false;
    }

    let _ = crate::ipc::process::terminate_gracefully(pid);
    let graceful_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < graceful_deadline {
        if !crate::ipc::process::is_alive(pid) && !crate::proxy_setup::is_proxy_reachable(port) {
            if !quiet {
                eprintln!(
                    "  Handed port {port} from standalone proxy PID {pid} to managed service."
                );
            }
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if crate::ipc::process::is_alive(pid) {
        let _ = crate::ipc::process::force_kill(pid);
    }
    let force_deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < force_deadline {
        if !crate::proxy_setup::is_proxy_reachable(port) {
            if !quiet {
                eprintln!(
                    "  Handed port {port} from standalone proxy PID {pid} to managed service."
                );
            }
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if !quiet {
        eprintln!("  Refusing managed proxy startup: port {port} was not released by PID {pid}.");
    }
    false
}

pub fn install(port: u16, quiet: bool) -> bool {
    let binary = find_binary();
    if binary.is_empty() {
        if !quiet {
            tracing::error!("Cannot find sobre binary for autostart");
        }
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        install_launchagent(&binary, port, quiet)
    }

    #[cfg(target_os = "linux")]
    {
        install_systemd(&binary, port, quiet)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (&binary, quiet);
        println!("  Autostart not supported on this platform");
        println!("  Run manually: sobre proxy start --port={port}");
        false
    }
}

/// Stops sobre's proxy wherever it runs: the autostart service, then the
/// proxy answering on the configured port and on the port the last one was
/// started on (`proxy start --port N`, or one an MCP server spawned). Only a
/// process that identifies itself as sobre's proxy is signalled.
pub fn stop_everywhere(quiet: bool) {
    stop();
    let mut ports = vec![crate::proxy_setup::default_port()];
    if let Some(port) = crate::proxy_setup::last_started_port()
        && !ports.contains(&port)
    {
        ports.push(port);
    }
    for port in ports {
        if let Some(pid) = stop_proxy_on_port(port)
            && !quiet
        {
            println!("  ✓ Proxy stopped (PID {pid}, port {port})");
        }
    }
}

pub fn stop() {
    #[cfg(target_os = "macos")]
    {
        let plist_path = launchagent_path();
        if plist_path.exists() {
            crate::core::launchd::bootout(PLIST_LABEL, &plist_path);
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", SYSTEMD_SERVICE])
            .output();
    }
}

/// Start the installed manager after atomically taking ownership of `port`.
/// This also handles the case where an MCP server spawned a standalone
/// proxy while the manager was temporarily unloaded.
pub fn start_on_port(port: u16) -> bool {
    #[cfg(target_os = "macos")]
    {
        let plist_path = launchagent_path();
        if plist_path.exists() {
            crate::core::launchd::bootout(PLIST_LABEL, &plist_path);
            if !release_proxy_port(port, false) {
                return false;
            }
            return crate::core::launchd::bootstrap(PLIST_LABEL, &plist_path);
        }
        false
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", SYSTEMD_SERVICE])
            .output();
        if release_proxy_port(port, false) {
            std::process::Command::new("systemctl")
                .args(["--user", "start", SYSTEMD_SERVICE])
                .status()
                .is_ok_and(|status| status.success())
        } else {
            false
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = port;
        false
    }
}

pub fn uninstall(_quiet: bool) {
    #[cfg(target_os = "macos")]
    uninstall_launchagent(_quiet);

    #[cfg(target_os = "linux")]
    uninstall_systemd(_quiet);
}

/// Whether this platform has a proxy-autostart backend (LaunchAgent on macOS,
/// systemd user service on Linux). Windows and other targets have none, so a
/// missing autostart there must not be treated as a failure by `doctor`.
pub fn is_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

/// Returns true if the proxy autostart is installed (plist/systemd service file exists).
pub fn is_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        launchagent_path().exists()
    }
    #[cfg(target_os = "linux")]
    {
        systemd_path().exists()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Returns true when the installed manager is actively responsible for the proxy.
/// Callers must not spawn a second foreground proxy while this is true.
pub fn is_loaded() -> bool {
    #[cfg(target_os = "macos")]
    {
        is_installed() && crate::core::launchd::is_loaded(PLIST_LABEL)
    }
    #[cfg(target_os = "linux")]
    {
        is_installed()
            && std::process::Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", SYSTEMD_SERVICE])
                .status()
                .is_ok_and(|status| status.success())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn launchagent_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Library/LaunchAgents")
        .join(format!("{PLIST_LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn install_launchagent(binary: &str, port: u16, quiet: bool) -> bool {
    let plist_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Library/LaunchAgents");
    let _ = std::fs::create_dir_all(&plist_dir);

    let plist_path = plist_dir.join(format!("{PLIST_LABEL}.plist"));
    // Stop the registered job first, then take over any standalone proxy an
    // MCP server may have spawned while launchd was unloaded. Starting the
    // new job with an occupied port causes an unconditional KeepAlive loop.
    crate::core::launchd::bootout(PLIST_LABEL, &plist_path);
    if !release_proxy_port(port, quiet) {
        return false;
    }
    // Proxy logs are STATE — $XDG_STATE_HOME/sobre/logs, never a shared
    // temporary directory.
    let Ok(state_dir) = crate::core::paths::state_dir() else {
        if !quiet {
            eprintln!("Cannot resolve sobre's state directory for the proxy logs");
        }
        return false;
    };
    let log_dir = state_dir.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    // Wrap the launchd invocation in a deny-~/Documents seatbelt sandbox
    // so the proxy (a TCC-standalone process) can never trip the privacy prompt.
    let port_arg = format!("--port={port}");
    let program_args = crate::core::tcc_guard_sandbox::program_args_xml(
        &crate::core::tcc_guard_sandbox::wrap_launchd_args(binary, &["proxy", "start", &port_arg]),
        "        ",
    );

    // Pin the directory layout. A launchd-spawned proxy inherits only
    // launchd's minimal environment (no HOME, no XDG vars), so it resolves a
    // *different* config/data dir than the CLI that installed it — it never sees
    // the user's config.toml edits (live-upstream reload reads nothing) and
    // derives a mismatched session token. Bake the exact dirs this CLI resolves
    // into the plist so the managed proxy always agrees with the CLI.
    let env_vars = crate::core::tcc_guard_sandbox::pinned_layout_env_xml();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{PLIST_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{program_args}
    </array>
{env_vars}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>"#,
        stdout = log_dir.join("proxy.stdout.log").display(),
        stderr = log_dir.join("proxy.stderr.log").display(),
    );

    let _ = std::fs::write(&plist_path, &plist);

    let ok = crate::core::launchd::bootstrap(PLIST_LABEL, &plist_path);

    if !quiet {
        if ok {
            println!("  Installed LaunchAgent: {}", plist_path.display());
            println!("  Proxy will start on login and restart if stopped");
        } else {
            println!("  Created LaunchAgent at {}", plist_path.display());
            println!("  Load reported a problem; check: launchctl print {PLIST_LABEL}");
        }
    }
    ok
}

#[cfg(target_os = "macos")]
fn uninstall_launchagent(quiet: bool) {
    let plist_path = launchagent_path();
    if !plist_path.exists() {
        if !quiet {
            println!("  LaunchAgent not installed, nothing to remove");
        }
        return;
    }

    crate::core::launchd::bootout(PLIST_LABEL, &plist_path);

    let _ = std::fs::remove_file(&plist_path);
    if !quiet {
        println!("  Removed LaunchAgent: {}", plist_path.display());
    }
}

#[cfg(target_os = "linux")]
fn systemd_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config/systemd/user")
        .join(format!("{SYSTEMD_SERVICE}.service"))
}

/// The unit's `Environment=` lines: the directory layout this CLI resolves,
/// so the proxy systemd starts reads the same config and token (the plist
/// does the same on macOS).
#[cfg(target_os = "linux")]
fn systemd_layout_env() -> String {
    let mut entries: Vec<(&str, String)> = vec![("RUST_LOG", "info".to_string())];
    if let Some(home) = dirs::home_dir() {
        entries.push(("HOME", home.display().to_string()));
    }
    for (key, dir) in [
        ("SOBRE_CONFIG_DIR", crate::core::paths::config_dir()),
        ("SOBRE_DATA_DIR", crate::core::paths::data_dir()),
        ("SOBRE_STATE_DIR", crate::core::paths::state_dir()),
        ("SOBRE_CACHE_DIR", crate::core::paths::cache_dir()),
    ] {
        if let Ok(path) = dir {
            entries.push((key, path.display().to_string()));
        }
    }
    entries.iter().fold(String::new(), |mut out, (key, value)| {
        out.push_str(&format!(
            "Environment=\"{key}={}\"\n",
            systemd_escape(value)
        ));
        out
    })
}

/// Escapes `"` and `\` inside a double-quoted systemd value.
#[cfg(target_os = "linux")]
fn systemd_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Removes the unit file and the `default.target.wants` link `enable` made,
/// for when the service could not start (no user session bus).
#[cfg(target_os = "linux")]
fn remove_systemd_unit(service_path: &std::path::Path) {
    let _ = std::fs::remove_file(service_path);
    if let Some(dir) = service_path.parent() {
        let wants = dir.join("default.target.wants");
        let _ = std::fs::remove_file(wants.join(format!("{SYSTEMD_SERVICE}.service")));
        let _ = std::fs::remove_dir(&wants);
        let _ = std::fs::remove_dir(dir);
        if let Some(systemd) = dir.parent() {
            let _ = std::fs::remove_dir(systemd);
        }
    }
}

#[cfg(target_os = "linux")]
fn install_systemd(binary: &str, port: u16, quiet: bool) -> bool {
    let Some(home) = dirs::home_dir() else {
        if !quiet {
            println!("  Cannot resolve the home directory for the systemd user service");
        }
        return false;
    };
    let service_dir = home.join(".config/systemd/user");
    let _ = std::fs::create_dir_all(&service_dir);

    let service_path = service_dir.join(format!("{SYSTEMD_SERVICE}.service"));

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", SYSTEMD_SERVICE])
        .output();
    if !release_proxy_port(port, quiet) {
        return false;
    }

    let unit = format!(
        r#"[Unit]
Description=sobre API Proxy
After=network.target
StartLimitIntervalSec=300
StartLimitBurst=5

[Service]
Type=simple
ExecStart="{binary}" proxy start --port={port}
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal
{env}
[Install]
WantedBy=default.target
"#,
        binary = systemd_escape(binary),
        env = systemd_layout_env(),
    );

    let _ = std::fs::write(&service_path, &unit);

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    let result = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", SYSTEMD_SERVICE])
        .output();

    let started = result.as_ref().is_ok_and(|output| output.status.success());
    if !started {
        // Nothing half-installed stays behind: no unit, no wants link.
        remove_systemd_unit(&service_path);
    }
    if !quiet {
        match &result {
            Ok(o) if o.status.success() => {
                println!("  Installed systemd user service: {SYSTEMD_SERVICE}");
                println!("  Proxy will start on login and restart if stopped");
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                println!("  systemd could not start the service: {}", err.trim());
            }
            Err(e) => println!("  Could not run systemctl: {e}"),
        }
    }
    started
}

#[cfg(target_os = "linux")]
fn uninstall_systemd(quiet: bool) {
    let service_path = systemd_path();
    if !service_path.exists() {
        if !quiet {
            println!("  systemd service not installed, nothing to remove");
        }
        return;
    }

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", SYSTEMD_SERVICE])
        .output();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", SYSTEMD_SERVICE])
        .output();
    remove_systemd_unit(&service_path);
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    if !quiet {
        println!("  Removed systemd service: {SYSTEMD_SERVICE}");
    }
}

pub fn find_binary() -> String {
    crate::core::portable_binary::resolve_portable_binary()
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use super::release_proxy_port;
    use super::{health_identifies_sobre_proxy, proxy_pid_from_health};
    #[cfg(unix)]
    use crate::proxy_setup::is_proxy_reachable as port_is_open;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::io::{Read, Write};
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::net::TcpListener;

    #[test]
    fn health_pid_parser_accepts_only_well_formed_health() {
        assert_eq!(
            proxy_pid_from_health(r#"{"status":"ok","pid":4242}"#),
            Some(4242)
        );
        assert_eq!(
            proxy_pid_from_health(r#"{"status":"busy","pid":4242}"#),
            None
        );
        assert_eq!(proxy_pid_from_health(r#"{"status":"ok","pid":0}"#), None);
        assert_eq!(proxy_pid_from_health(r#"{"status":"ok"}"#), None);
        assert_eq!(proxy_pid_from_health("not json"), None);
        assert!(health_identifies_sobre_proxy(
            r#"{"status":"ok","service":"sobre-proxy","pid":4242}"#
        ));
        assert!(!health_identifies_sobre_proxy(
            r#"{"status":"ok","pid":4242}"#
        ));
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn managed_handoff_fails_closed_for_unidentified_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                    .unwrap();
                let mut request = [0_u8; 512];
                if stream.read(&mut request).unwrap_or(0) > 0 {
                    let body = r#"{"status":"ok"}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
            }
        });

        assert!(!release_proxy_port(port, true));
        server.join().unwrap();
    }

    #[cfg(unix)]
    struct ChildGuard(std::process::Child);

    #[cfg(unix)]
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[cfg(unix)]
    fn managed_handoff_stops_identified_proxy_and_releases_port() {
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);

        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "proxy_autostart::tests::managed_handoff_test_child",
                "--nocapture",
            ])
            .env("SOBRE_HANDOFF_TEST_PORT", port.to_string())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);

        let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !port_is_open(port) && std::time::Instant::now() < ready_deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(port_is_open(port), "test proxy did not bind port {port}");
        assert!(release_proxy_port(port, true));
        let status = child.0.wait().unwrap();
        assert!(
            !status.success(),
            "handoff must terminate the standalone proxy"
        );
        assert!(!port_is_open(port), "handoff must release port {port}");
    }

    #[test]
    #[ignore = "helper process for managed_handoff_stops_identified_proxy_and_releases_port"]
    #[cfg(unix)]
    fn managed_handoff_test_child() {
        let port: u16 = std::env::var("SOBRE_HANDOFF_TEST_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        loop {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let mut request = [0_u8; 512];
            if stream.read(&mut request).unwrap_or(0) > 0 {
                let body = format!(
                    r#"{{"status":"ok","service":"sobre-proxy","pid":{}}}"#,
                    std::process::id()
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        }
    }
}
