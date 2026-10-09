//! Codex CLI config.toml proxy wiring.

use std::path::Path;

use toml_edit::{DocumentMut, Item, RawString};

use super::util::is_proxy_reachable;
use super::wiring;

/// The wiring slot of Codex's top-level `openai_base_url`.
const SLOT: &str = "codex";

fn codex_config_path(home: &Path) -> std::path::PathBuf {
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    crate::core::home::resolve_codex_config_path().unwrap_or_else(|| codex_dir.join("config.toml"))
}

/// Whether Codex's `openai_base_url` is the one sobre wrote.
pub(crate) fn codex_points_at_proxy(home: &Path) -> bool {
    std::fs::read_to_string(codex_config_path(home))
        .is_ok_and(|content| codex_config_has_local_proxy_entry(&content))
}

/// Takes Codex off the proxy when sobre put it there. Returns whether it
/// changed anything.
pub(crate) fn uninstall_codex_env(home: &Path, quiet: bool) -> bool {
    let config_path = codex_config_path(home);
    let existing = match std::fs::read_to_string(&config_path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return false,
    };
    if !codex_config_has_local_proxy_entry(&existing) {
        return false;
    }
    let cleaned = strip_codex_proxy_entries(&existing);
    let _ = crate::config_io::write_atomic(&config_path, &cleaned);
    wiring::forget(SLOT);
    if !quiet {
        println!("  ✓ Removed Codex proxy URL(s) from Codex CLI config");
    }
    true
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

    // The first edit of a file the user had keeps a `.sobre.bak` of it, like
    // every other agent file sobre edits.
    let _ = crate::config_io::write_atomic_with_backup(config_path, &updated);
    match entries.first() {
        Some((_, url)) if updated.contains(url.as_str()) => wiring::record(SLOT, url, None),
        _ => {
            wiring::forget(SLOT);
        }
    }
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

/// The top-level key Codex reads its built-in OpenAI provider's base URL from.
const BASE_URL_KEY: &str = "openai_base_url";

/// Point Codex's built-in OpenAI provider at `value` via the documented top-level
/// `openai_base_url` key, replacing the value sobre wrote earlier. An
/// `openai_base_url` the user configured — remote or local — is preserved
/// and never overwritten. A new key goes first in the file, ahead of the
/// first `[table]`, so Codex reads it as a top-level key. Everything else
/// stays as written; a file that is not valid TOML is returned unchanged.
pub(crate) fn render_codex_config(existing: &str, entries: &[(&str, String)]) -> String {
    let Ok(mut doc) = existing.parse::<DocumentMut>() else {
        return existing.to_string();
    };
    remove_proxy_entry(&mut doc);
    for (key, value) in entries {
        // `remove_proxy_entry` left only the user's own value.
        if !doc.contains_key(key) {
            doc.insert(key, toml_edit::value(value.as_str()));
            doc.sort_values_by(|a, _, b, _| (b.get() == *key).cmp(&(a.get() == *key)));
        }
    }
    doc.to_string()
}

/// Remove sobre's own Codex proxy entry from a `config.toml` body: a local
/// top-level `openai_base_url`. Custom endpoints, tables, comments and layout
/// are preserved; a body that is not valid TOML is returned unchanged.
pub(crate) fn strip_codex_proxy_entries(body: &str) -> String {
    let Ok(mut doc) = body.parse::<DocumentMut>() else {
        return body.to_string();
    };
    remove_proxy_entry(&mut doc);
    doc.to_string()
}

/// Removes sobre's top-level `openai_base_url`. Comments above it move to the
/// top of the file, where sobre writes the key, so none is lost.
fn remove_proxy_entry(doc: &mut DocumentMut) {
    if !holds_proxy_entry(doc) {
        return;
    }
    let Some((key, _)) = doc.remove_entry(BASE_URL_KEY) else {
        return;
    };
    let above = key.leaf_decor().prefix().and_then(RawString::as_str);
    if let Some(above) = above.filter(|text| text.contains('#')) {
        let top = doc.decor().prefix().and_then(RawString::as_str);
        let top = format!("{above}{}", top.unwrap_or_default());
        doc.decor_mut().set_prefix(top);
    }
}

/// Whether the top-level `openai_base_url` is the URL sobre wrote.
fn holds_proxy_entry(doc: &DocumentMut) -> bool {
    doc.get(BASE_URL_KEY)
        .and_then(Item::as_str)
        .is_some_and(|url| wiring::is_ours(SLOT, url))
}

pub(crate) fn codex_config_has_local_proxy_entry(body: &str) -> bool {
    body.parse::<DocumentMut>()
        .is_ok_and(|doc| holds_proxy_entry(&doc))
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
