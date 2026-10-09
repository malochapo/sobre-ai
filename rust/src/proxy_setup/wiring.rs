//! What sobre wrote into an agent's configuration for the proxy, and the
//! value it replaced, kept in `<state dir>/proxy_wiring.json`.
//!
//! `proxy disable`, `proxy cleanup`, `doctor --fix` and `uninstall` undo
//! exactly that. A URL is sobre's only when it is the one recorded for its
//! slot or, with no record, the proxy's own address on its port: another
//! tool's local endpoint (LiteLLM, Ollama) is never taken for the proxy, and
//! a value the proxy replaced comes back as it was.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One wired setting: Claude Code's `ANTHROPIC_BASE_URL` (`claude`), Codex's
/// `openai_base_url` (`codex`), an OpenCode provider's `baseURL`
/// (`opencode.anthropic`, `opencode.openai`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    /// The URL sobre wrote.
    pub wrote: String,
    /// The value it replaced, `None` when the setting was absent.
    pub displaced: Option<String>,
}

fn path() -> Option<PathBuf> {
    crate::core::paths::state_dir()
        .ok()
        .map(|d| d.join("proxy_wiring.json"))
}

fn load() -> BTreeMap<String, Record> {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(records: &BTreeMap<String, Record>) {
    let Some(path) = path() else {
        return;
    };
    if records.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Ok(json) = serde_json::to_string_pretty(records) {
        let _ = crate::config_io::write_atomic(&path, &json);
    }
}

/// The record of `slot`, if sobre wired it.
pub(crate) fn recorded(slot: &str) -> Option<Record> {
    load().remove(slot)
}

/// Records that sobre wrote `wrote` into `slot`, replacing `displaced`. A
/// slot already wired (the proxy moved to another port) keeps the value it
/// first replaced.
pub(crate) fn record(slot: &str, wrote: &str, displaced: Option<&str>) {
    let mut records = load();
    let displaced = match records.get(slot) {
        Some(previous) => previous.displaced.clone(),
        None => displaced.map(str::to_string),
    };
    records.insert(
        slot.to_string(),
        Record {
            wrote: wrote.to_string(),
            displaced,
        },
    );
    save(&records);
}

/// Forgets `slot` once its setting is undone; returns the record it held.
pub(crate) fn forget(slot: &str) -> Option<Record> {
    let mut records = load();
    let removed = records.remove(slot);
    if removed.is_some() {
        save(&records);
    }
    removed
}

/// Whether `value`, found in `slot`, is the URL sobre wrote there.
pub(crate) fn is_ours(slot: &str, value: &str) -> bool {
    match recorded(slot) {
        Some(record) => same_url(&record.wrote, value),
        None => is_proxy_address(value),
    }
}

/// Whether `url` is the proxy's own address — `http://127.0.0.1:<port>`,
/// with or without `/v1` — on the configured port or on the port the last
/// proxy was started on.
pub(crate) fn is_proxy_address(url: &str) -> bool {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix("/v1").unwrap_or(url);
    let Some(port) = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|p| p.parse::<u16>().ok())
    else {
        return false;
    };
    port == super::default_port() || super::last_started_port() == Some(port)
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_decides_and_keeps_the_first_displaced_value() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        assert!(!is_ours("claude", "http://127.0.0.1:1"));
        record("claude", "http://127.0.0.1:1", Some("https://mine.example"));
        assert!(is_ours("claude", "http://127.0.0.1:1/"));
        assert!(!is_ours("claude", "http://localhost:11434"));
        // The proxy moved: the URL changes, the replaced value stays.
        record("claude", "http://127.0.0.1:2", None);
        assert_eq!(
            forget("claude"),
            Some(Record {
                wrote: "http://127.0.0.1:2".into(),
                displaced: Some("https://mine.example".into()),
            })
        );
        assert_eq!(recorded("claude"), None);
    }

    #[test]
    fn without_a_record_only_the_proxys_own_address_is_ours() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let port = super::super::default_port();
        assert!(is_proxy_address(&format!("http://127.0.0.1:{port}")));
        assert!(is_proxy_address(&format!("http://127.0.0.1:{port}/v1")));
        for theirs in [
            "http://localhost:11434",
            "http://127.0.0.1:11434",
            "http://127.0.0.1:4000/v1",
            "https://api.anthropic.com",
        ] {
            if theirs.ends_with(&format!(":{port}")) {
                continue;
            }
            assert!(!is_proxy_address(theirs), "{theirs}");
        }
    }
}
