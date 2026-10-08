//! Codex CLI config.toml proxy wiring.

use std::path::Path;

use super::util::is_proxy_reachable;

pub(crate) fn uninstall_codex_env(home: &Path, quiet: bool) {
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    let config_path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| codex_dir.join("config.toml"));
    let existing = match std::fs::read_to_string(&config_path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return,
    };

    let has_local = codex_config_has_local_proxy_entry(&existing);
    if !has_local {
        return;
    }

    let cleaned = strip_codex_proxy_entries(&existing);
    let _ = std::fs::write(&config_path, &cleaned);
    if !quiet {
        println!("  ✓ Removed Codex proxy URL(s) from Codex CLI config");
    }
}
/// (Re)apply ONLY the Codex CLI proxy env from the current config.
pub(crate) fn install_codex_env(home: &Path, port: u16, quiet: bool) {
    let config_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    let mode = if codex_uses_chatgpt_login(home) {
        CodexProxyMode::ChatGpt
    } else {
        CodexProxyMode::ApiKey
    };
    let config_path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| config_dir.join("config.toml"));
    install_codex_env_at_path(&config_path, port, quiet, mode);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexProxyMode {
    ApiKey,
    ChatGpt,
}

/// Testable core of `install_codex_env`: operates on an explicit Codex config
/// directory instead of resolving it from `CODEX_HOME` / the real home.
#[cfg(test)]
pub(crate) fn install_codex_env_at(config_dir: &Path, port: u16, quiet: bool) {
    install_codex_env_at_mode(config_dir, port, quiet, CodexProxyMode::ApiKey);
}

#[cfg(test)]
pub(crate) fn install_codex_env_at_mode(
    config_dir: &Path,
    port: u16,
    quiet: bool,
    mode: CodexProxyMode,
) {
    install_codex_env_at_path(&config_dir.join("config.toml"), port, quiet, mode);
}

fn install_codex_env_at_path(config_path: &Path, port: u16, quiet: bool, mode: CodexProxyMode) {
    // API-key Codex goes through the proxy's `/v1` rail: Codex reads the
    // built-in OpenAI provider's base URL from the top-level `openai_base_url`
    // key (openai/codex#12031).
    //
    // A ChatGPT login talks to chatgpt.com directly and is left alone; an
    // empty `entries` still lets `render_codex_config` strip a local
    // `openai_base_url` left from API-key mode.
    let base = format!("http://127.0.0.1:{port}");
    let entries: Vec<(&str, String)> = match mode {
        CodexProxyMode::ApiKey => vec![("openai_base_url", format!("{base}/v1"))],
        CodexProxyMode::ChatGpt => Vec::new(),
    };

    // Writing a proxy URL only makes sense against a live proxy.
    if !entries.is_empty() && !is_proxy_reachable(port) {
        if !quiet {
            println!("  Skipping Codex CLI proxy env (proxy not running on port {port})");
        }
        return;
    }

    if !config_path.parent().is_some_and(std::path::Path::exists) {
        return;
    }

    let existing = std::fs::read_to_string(config_path).unwrap_or_default();
    let updated = render_codex_config(&existing, &entries);

    if updated == existing {
        if !quiet {
            if entries.is_empty() {
                println!("  Codex ChatGPT login — config left native (no sobre proxy entries)");
            } else {
                println!("  Codex CLI proxy env already configured");
            }
        }
        return;
    }

    let _ = std::fs::write(config_path, &updated);
    if !quiet {
        match mode {
            CodexProxyMode::ApiKey => {
                println!("  Configured openai_base_url in Codex CLI config");
            }
            CodexProxyMode::ChatGpt => println!(
                "  Codex ChatGPT login — removed stale sobre proxy entries (Codex now talks directly to ChatGPT)"
            ),
        }
    }
}

/// Point Codex's built-in OpenAI provider at `value` via the documented top-level
/// `openai_base_url` key, replacing a stale local value with the canonical
/// one. A custom *remote* `openai_base_url` the user configured is preserved and
/// never overwritten in API-key mode (#366). Keys are emitted as top-level keys
/// (before the first `[table]`) so Codex actually reads them.
pub(crate) fn render_codex_config(existing: &str, entries: &[(&str, String)]) -> String {
    let cleaned = strip_codex_proxy_entries(existing);

    let mut prefix = String::new();
    for (key, value) in entries {
        let has_remote_key = has_top_level_codex_config_key(&cleaned, key, |t| {
            !(t.contains("127.0.0.1") || t.contains("localhost"))
        });
        if !has_remote_key {
            prefix.push_str(&format!("{key} = \"{value}\"\n"));
        }
    }
    if prefix.is_empty() {
        cleaned
    } else {
        // `strip_codex_proxy_entries` already dropped local keys, so prepend fresh
        // top-level keys ahead of every existing line.
        format!("{prefix}{cleaned}")
    }
}

/// Remove sobre's own Codex proxy entry from a `config.toml` body: a local
/// top-level `openai_base_url`. Custom remote endpoints and tables are preserved.
pub(crate) fn strip_codex_proxy_entries(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut current_table: Option<&str> = None;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if lines[i].trim_start().starts_with('[') {
            current_table = Some(trimmed);
            kept.push(lines[i]);
            i += 1;
            continue;
        }

        if should_strip_codex_proxy_entry(lines[i].trim_start(), current_table) {
            i += 1;
            continue;
        }

        kept.push(lines[i]);
        i += 1;
    }

    let out = kept;

    let mut s = out.join("\n");
    while s.contains("\n\n\n") {
        s = s.replace("\n\n\n", "\n\n");
    }
    let s = s.trim_end_matches('\n');
    if s.is_empty() {
        String::new()
    } else {
        format!("{s}\n")
    }
}

pub(crate) fn has_top_level_codex_config_key(
    body: &str,
    key: &str,
    predicate: impl Fn(&str) -> bool,
) -> bool {
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            break;
        }
        if toml_assignment_key(t) == Some(key) && predicate(t) {
            return true;
        }
    }
    false
}

pub(crate) fn should_strip_codex_proxy_entry(t: &str, current_table: Option<&str>) -> bool {
    match current_table {
        None => is_local_codex_base_url_entry(t, &["openai_base_url"]),
        _ => false,
    }
}

pub(crate) fn is_local_codex_base_url_entry(t: &str, keys: &[&str]) -> bool {
    toml_assignment_key(t).is_some_and(|key| keys.contains(&key))
        && (t.contains("127.0.0.1") || t.contains("localhost"))
}

pub(crate) fn toml_assignment_key(t: &str) -> Option<&str> {
    let key = t.split_once('=')?.0.trim();
    if key.is_empty() || key.starts_with('#') {
        None
    } else {
        Some(key)
    }
}

pub(crate) fn codex_config_has_local_proxy_entry(body: &str) -> bool {
    body.lines()
        .map(str::trim_start)
        .take_while(|t| !t.starts_with('['))
        .any(|t| is_local_codex_base_url_entry(t, &["openai_base_url"]))
}

/// True when Codex will authenticate via a **ChatGPT login** (OAuth) rather than
/// an API key. Codex takes its model credential from `CODEX_API_KEY` or from
/// `auth.json`; it does not read `OPENAI_API_KEY`, so that variable never
/// overrides a stored ChatGPT login.
pub(crate) fn codex_uses_chatgpt_login(home: &Path) -> bool {
    if std::env::var("CODEX_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return false;
    }
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    auth_is_chatgpt(&codex_dir)
}

/// True when `<codex_dir>/auth.json` records a ChatGPT/backend auth mode.
/// False when the file is missing, unreadable, or in API-key mode.
pub(crate) fn auth_is_chatgpt(codex_dir: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(codex_dir.join("auth.json")) else {
        return false;
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let Some(mode) = doc.get("auth_mode").and_then(|v| v.as_str()) else {
        return false;
    };
    let normalized = mode
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "chatgpt" | "chatgptauthtokens" | "personalaccesstoken" | "agentidentity"
    )
}
