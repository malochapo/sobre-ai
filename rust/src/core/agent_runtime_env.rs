//! Bridge agent runtime/session environment variables across sobre processes.
//!
//! The sobre MCP server is a long-lived child of the agent host. Some agents
//! (notably Codex) expose runtime/session variables such as `CODEX_THREAD_ID`
//! only in the *native agent shell* environment, not in the MCP server process.
//! `sobre_shell` runs inside the MCP server, so it cannot forward those
//! variables by reading its own `std::env`.
//!
//! Short-lived sobre processes that *do* run inside the agent environment —
//! the hook handlers (`sobre hook …`) and the `sobre -c` shell wrapper —
//! [`capture`] the relevant variables into a small file in the data dir. The MCP
//! server then [`load`]s them when constructing the child environment for
//! `sobre_shell` (see `crate::server::execute`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Env var name prefixes identifying agent runtime/session state worth forwarding
/// to `sobre_shell` child processes.
pub const FORWARD_PREFIXES: &[&str] = &["CODEX_", "CLAUDE_", "OPENCODE_", "GEMINI_"];

/// Case-insensitive name substrings that mark an env var as credential-shaped.
///
/// A variable whose name contains any of these is NEVER forwarded to `sobre_shell`
/// children — and never captured to disk — even when it matches a forwardable
/// prefix. Forwarding API keys / tokens / passwords into every child shell is an
/// exfiltration risk that output redaction cannot stop: a `curl … -d "$KEY"`
/// child never prints the secret to stdout, so the redactor never sees it (GH
/// security audit, finding 2). Only non-secret session/thread identifiers
/// (`*_THREAD_ID`, `*_SESSION`, …) should cross the bridge.
const CREDENTIAL_MARKERS: &[&str] = &[
    "_KEY",       // *_API_KEY, *_ACCESS_KEY, *_PRIVATE_KEY, *_SECRET_KEY
    "APIKEY",     // unseparated spelling
    "SECRET",     // *_SECRET, *_CLIENT_SECRET
    "TOKEN",      // *_TOKEN, *_ACCESS_TOKEN, *_REFRESH_TOKEN
    "PASSWORD",   // *_PASSWORD, *_SERVER_PASSWORD
    "PASSWD",     // unseparated spelling
    "CREDENTIAL", // *_CREDENTIAL(S)
    "AUTH",       // *_AUTH, *_OAUTH, *_AUTHORIZATION
];

/// Whether `key` looks like a secret/credential that must never be forwarded or
/// persisted, regardless of any matching forwardable prefix.
#[must_use]
pub fn is_credential_shaped(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    CREDENTIAL_MARKERS
        .iter()
        .any(|marker| upper.contains(marker))
}

const FILE_NAME: &str = "agent_runtime_env.json";

/// Captured variables older than this are ignored: a stale session/thread id is
/// worse than forwarding none, and a fresh session re-captures on its first hook.
const TTL_SECS: u64 = 7_200;

/// Whether `key` is an agent runtime variable sobre forwards to child shells.
///
/// A variable qualifies only when it (1) matches a forwardable agent prefix AND
/// (2) is not credential-shaped — session/thread identifiers cross the bridge,
/// secrets never do (GH security audit, finding 2).
#[must_use]
pub fn is_forwardable(key: &str) -> bool {
    FORWARD_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
        && !is_credential_shaped(key)
}

/// Canonical capture-file location: the STATE dir.
///
/// This file holds captured agent **session/thread identifiers** (e.g.
/// `CODEX_THREAD_ID`); credential-shaped vars are filtered out by
/// [`is_forwardable`] and never written here (GH security audit, finding 2). It
/// still lives in the RW state category — never the RO/shareable config dir —
/// and is always written `0o600` as defence-in-depth.
fn store_path() -> Option<PathBuf> {
    crate::core::paths::state_dir()
        .ok()
        .map(|dir| dir.join(FILE_NAME))
}

/// Forwardable variables present in the current process environment.
#[must_use]
pub fn collect_from_process() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(key, _)| is_forwardable(key))
        .collect()
}

fn read_store(path: &Path) -> Option<(BTreeMap<String, String>, u64)> {
    let content = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let captured_at = value
        .get("captured_at")
        .and_then(serde_json::Value::as_u64)?;
    let vars = value
        .get("vars")
        .and_then(serde_json::Value::as_object)?
        .iter()
        .filter_map(|(key, val)| val.as_str().map(|s| (key.clone(), s.to_string())))
        .collect();
    Some((vars, captured_at))
}

/// Capture forwardable variables from the current (agent) environment into the
/// data dir so the MCP server can forward them to `sobre_shell` children.
///
/// No-op when the current environment carries no forwardable variables — this
/// prevents a process with a stripped environment (e.g. the MCP server itself)
/// from clobbering a good capture. The file is rewritten when the variable set
/// changes, and when the same set's capture is a quarter of the TTL old, so a
/// session that keeps running commands never sees its capture expire.
pub fn capture() {
    let vars = collect_from_process();
    if vars.is_empty() {
        return;
    }
    let Some(path) = store_path() else {
        return;
    };
    let now = crate::core::clock::now_unix();
    if let Some((existing, captured_at)) = read_store(&path)
        && existing == vars
        && now.saturating_sub(captured_at) < TTL_SECS / 4
    {
        return;
    }
    let payload = serde_json::json!({ "vars": vars, "captured_at": now });
    let Ok(json) = serde_json::to_string_pretty(&payload) else {
        return;
    };
    // Atomic write + `0o600` (owner-only): captured keys must never be
    // group/world-readable. `write_atomic` also rejects symlinks and creates
    // the state dir if needed.
    let _ = crate::config_io::write_atomic(&path, &json);
}

/// Load captured agent runtime variables, honoring the freshness TTL.
///
/// When no valid capture exists, falls back to probing the parent process's
/// environment. This covers the case where the sobre MCP server was
/// spawned by an agent that sets runtime vars only in native shell commands
/// (e.g. Codex with CODEX_THREAD_ID) without going through `sobre -c` first.
#[must_use]
pub fn load() -> BTreeMap<String, String> {
    let Some(path) = store_path() else {
        return BTreeMap::new();
    };
    let Some((vars, captured_at)) = read_store(&path) else {
        // No capture file — try probing the parent process environment.
        let probed = probe_parent_env();
        if !probed.is_empty() {
            persist_probed_vars(&path, &probed);
        }
        return probed;
    };
    if crate::core::clock::now_unix().saturating_sub(captured_at) > TTL_SECS {
        return BTreeMap::new();
    }
    // Defense-in-depth: never forward a credential-shaped var, even from a
    // hand-edited capture file.
    vars.into_iter()
        .filter(|(key, _)| is_forwardable(key))
        .collect()
}

/// Probe the parent process's environment for forwardable agent runtime
/// variables. On Linux reads `/proc/<ppid>/environ`; on macOS uses
/// `ps eww`. Returns only non-credential forwardable vars.
fn probe_parent_env() -> BTreeMap<String, String> {
    let ppid = parent_pid();
    if ppid == 0 {
        return BTreeMap::new();
    }
    let raw_env = read_parent_environ(ppid);
    raw_env
        .into_iter()
        .filter(|(key, _)| is_forwardable(key))
        .collect()
}

fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getppid() is always safe — it reads the kernel's ppid for
        // the calling process, has no side effects, and cannot fail.
        unsafe { libc::getppid() as u32 }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Read environment variables from a process. On Linux, reads
/// `/proc/<pid>/environ` (NUL-separated key=value pairs). On macOS, falls
/// back to parsing `ps eww <pid>` output.
fn read_parent_environ(pid: u32) -> BTreeMap<String, String> {
    // Linux: /proc/<pid>/environ is most reliable
    #[cfg(target_os = "linux")]
    if let Ok(data) = std::fs::read(format!("/proc/{pid}/environ")) {
        return parse_null_separated_env(&data);
    }

    // macOS/fallback: ps eww gives env in a less structured format
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("ps")
        .args(["eww", "-o", "command", "-p", &pid.to_string()])
        .output()
        && output.status.success()
    {
        return parse_ps_environ(&String::from_utf8_lossy(&output.stdout));
    }

    let _ = pid;
    BTreeMap::new()
}

#[cfg(target_os = "linux")]
fn parse_null_separated_env(data: &[u8]) -> BTreeMap<String, String> {
    data.split(|&b| b == 0)
        .filter_map(|entry| {
            let s = std::str::from_utf8(entry).ok()?;
            let (key, val) = s.split_once('=')?;
            Some((key.to_string(), val.to_string()))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn parse_ps_environ(output: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for line in output.lines().skip(1) {
        for token in line.split_whitespace() {
            if let Some((key, val)) = token.split_once('=')
                && FORWARD_PREFIXES
                    .iter()
                    .any(|prefix| key.starts_with(prefix))
            {
                result.insert(key.to_string(), val.to_string());
            }
        }
    }
    result
}

fn persist_probed_vars(path: &Path, vars: &BTreeMap<String, String>) {
    let payload =
        serde_json::json!({ "vars": vars, "captured_at": crate::core::clock::now_unix() });
    if let Ok(json) = serde_json::to_string_pretty(&payload) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = crate::config_io::write_atomic(path, &json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_forwardable_matches_known_prefixes() {
        assert!(is_forwardable("CODEX_THREAD_ID"));
        assert!(is_forwardable("CLAUDE_SESSION"));
        assert!(is_forwardable("OPENCODE_FOO"));
        assert!(!is_forwardable("PATH"));
        assert!(!is_forwardable("HOME"));
        assert!(!is_forwardable("SOBRE_DATA_DIR"));
    }

    #[test]
    fn capture_then_load_roundtrips() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::test_env::set_var("CODEX_THREAD_ID", "thread-roundtrip");

        capture();
        let loaded = load();

        crate::test_env::remove_var("CODEX_THREAD_ID");

        assert_eq!(
            loaded.get("CODEX_THREAD_ID").map(String::as_str),
            Some("thread-roundtrip")
        );
    }

    #[test]
    fn capture_is_noop_without_forwardable_vars() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        // Ensure no forwardable vars leak in from the host test environment.
        for (key, _) in collect_from_process() {
            crate::test_env::remove_var(key);
        }

        capture();
        let exists = store_path().is_some_and(|p| p.exists());

        assert!(!exists, "capture must not write a store with no vars");
    }

    #[test]
    fn load_ignores_expired_capture() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let path = store_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let stale = crate::core::clock::now_unix().saturating_sub(TTL_SECS + 60);
        let payload =
            serde_json::json!({ "vars": { "CODEX_THREAD_ID": "old" }, "captured_at": stale });
        std::fs::write(&path, serde_json::to_string_pretty(&payload).unwrap()).unwrap();

        let loaded = load();

        assert!(loaded.is_empty(), "expired capture must not be loaded");
    }

    /// J14: capturing the same variables again keeps the capture alive.
    #[test]
    fn an_old_capture_of_the_same_vars_is_refreshed() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        for (key, _) in collect_from_process() {
            crate::test_env::remove_var(key);
        }
        crate::test_env::set_var("CODEX_THREAD_ID", "same");
        let path = store_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = crate::core::clock::now_unix().saturating_sub(TTL_SECS - 60);
        let payload =
            serde_json::json!({ "vars": { "CODEX_THREAD_ID": "same" }, "captured_at": old });
        std::fs::write(&path, serde_json::to_string_pretty(&payload).unwrap()).unwrap();

        capture();
        let (_, captured_at) = read_store(&path).unwrap();
        crate::test_env::remove_var("CODEX_THREAD_ID");
        assert!(captured_at > old, "the capture must be refreshed");
    }

    #[cfg(unix)]
    #[test]
    fn capture_sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _iso = crate::core::data_dir::isolated_data_dir();
        for (key, _) in collect_from_process() {
            crate::test_env::remove_var(key);
        }
        // A forwardable (non-credential) var so a file is actually written; it
        // can still hold session ids, so owner-only perms remain a requirement.
        crate::test_env::set_var("CODEX_THREAD_ID", "session-id");

        capture();
        let path = store_path().unwrap();
        crate::test_env::remove_var("CODEX_THREAD_ID");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "captured runtime-env file must be owner-only"
        );
    }

    // Finding 2 (GH security audit): credential-shaped vars must never be
    // forwarded, even when they match a forwardable agent prefix.
    #[test]
    fn is_forwardable_rejects_credential_shaped_vars() {
        // Session/thread identifiers cross the bridge.
        assert!(is_forwardable("CODEX_THREAD_ID"));
        assert!(is_forwardable("CLAUDE_SESSION_ID"));
        assert!(is_forwardable("OPENCODE_SESSION"));
        // Secrets matching a forwardable prefix do NOT.
        assert!(!is_forwardable("GEMINI_API_KEY"));
        assert!(!is_forwardable("OPENCODE_API_KEY"));
        assert!(!is_forwardable("OPENCODE_SERVER_PASSWORD"));
        assert!(!is_forwardable("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(!is_forwardable("CODEX_ACCESS_TOKEN"));
        assert!(!is_forwardable("CODEX_CLIENT_SECRET"));
        assert!(!is_forwardable("CODEX_PRIVATE_KEY"));
        assert!(!is_forwardable("GEMINI_CREDENTIALS"));
    }

    #[test]
    fn capture_excludes_credentials() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        for (key, _) in collect_from_process() {
            crate::test_env::remove_var(key);
        }
        crate::test_env::set_var("CODEX_THREAD_ID", "thread-keep");
        crate::test_env::set_var("GEMINI_API_KEY", "secret-drop");

        capture();
        let loaded = load();

        crate::test_env::remove_var("CODEX_THREAD_ID");
        crate::test_env::remove_var("GEMINI_API_KEY");

        assert_eq!(
            loaded.get("CODEX_THREAD_ID").map(String::as_str),
            Some("thread-keep")
        );
        assert!(
            !loaded.contains_key("GEMINI_API_KEY"),
            "API key must never be captured or forwarded"
        );
    }

    #[test]
    fn load_never_forwards_credentials() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let path = store_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        // A capture file holding a real secret.
        let payload = serde_json::json!({
            "vars": { "CODEX_THREAD_ID": "t", "OPENCODE_SERVER_PASSWORD": "p4ssw0rd" },
            "captured_at": crate::core::clock::now_unix()
        });
        std::fs::write(&path, serde_json::to_string_pretty(&payload).unwrap()).unwrap();

        let loaded = load();

        assert!(
            !loaded.contains_key("OPENCODE_SERVER_PASSWORD"),
            "credential must not be loaded"
        );
        assert_eq!(loaded.get("CODEX_THREAD_ID").map(String::as_str), Some("t"));
    }

    #[test]
    fn store_path_is_under_state_dir_not_config_dir_when_split() {
        let _lock = crate::core::data_dir::test_env_lock();
        let state = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        crate::test_env::set_var("SOBRE_STATE_DIR", state.path());
        crate::test_env::set_var("SOBRE_CONFIG_DIR", config.path());

        let path = store_path().unwrap();

        crate::test_env::remove_var("SOBRE_STATE_DIR");
        crate::test_env::remove_var("SOBRE_CONFIG_DIR");

        assert!(
            path.starts_with(state.path()),
            "key file must live under the state dir: {}",
            path.display()
        );
        assert!(
            !path.starts_with(config.path()),
            "key file must never resolve under the config dir"
        );
    }
}
