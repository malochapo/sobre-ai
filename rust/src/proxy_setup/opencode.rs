//! OpenCode proxy wiring: `provider.<id>.options.baseURL` in
//! `~/.config/opencode/opencode.json`, OpenCode's own setting for a provider's
//! base URL.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::util::is_proxy_reachable;

/// The OpenCode providers the proxy serves, with the environment variable that
/// carries each one's API key.
const PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
];

fn config_path(home: &Path) -> PathBuf {
    home.join(".config/opencode/opencode.json")
}

/// The base URL sobre writes. Both SDKs expect the `/v1` prefix in it.
fn proxy_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

/// True when OpenCode authenticates `provider` with an API key: the key's
/// variable is set, or `opencode auth login` stored an `api` credential. An
/// OAuth login (Claude Pro/Max, ChatGPT) keeps talking to its provider.
fn uses_api_key(home: &Path, provider: &str, key_var: &str) -> bool {
    if std::env::var(key_var).is_ok_and(|v| !v.trim().is_empty()) {
        return true;
    }
    let auth = home.join(".local/share/opencode/auth.json");
    std::fs::read_to_string(auth)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|doc| doc.get(provider)?.get("type")?.as_str().map(str::to_string))
        .is_some_and(|kind| kind == "api")
}

fn base_url<'a>(doc: &'a Value, provider: &str) -> Option<&'a str> {
    doc.get("provider")?
        .get(provider)?
        .get("options")?
        .get("baseURL")?
        .as_str()
}

fn options_mut<'a>(doc: &'a mut Value, provider: &str) -> Option<&'a mut Map<String, Value>> {
    let mut node = doc;
    for key in ["provider", provider, "options"] {
        node = node
            .as_object_mut()?
            .entry(key)
            .or_insert_with(|| json!({}));
    }
    node.as_object_mut()
}

/// Removes `provider.<id>.options.baseURL` when it is `url`, then any object
/// that leaves empty. Returns whether it removed anything.
fn remove_base_url(doc: &mut Value, provider: &str, url: &str) -> bool {
    if base_url(doc, provider) != Some(url) {
        return false;
    }
    let path = ["provider", provider, "options", "baseURL"];
    for depth in (1..=path.len()).rev() {
        let parent = path[..depth - 1]
            .iter()
            .try_fold(&mut *doc, |node, key| node.get_mut(*key));
        let Some(Value::Object(parent)) = parent else {
            break;
        };
        let key = path[depth - 1];
        let empty = parent
            .get(key)
            .is_some_and(|v| v.as_object().is_some_and(Map::is_empty));
        if depth == path.len() || empty {
            parent.remove(key);
        } else {
            break;
        }
    }
    true
}

fn write(path: &Path, doc: &Value) {
    if let Ok(out) = serde_json::to_string_pretty(doc) {
        let _ = std::fs::write(path, out + "\n");
    }
}

/// Points each OpenCode provider that uses an API key at the proxy, and takes
/// one that no longer does off it. A base URL the user set is never replaced.
pub(crate) fn install_opencode_env(home: &Path, port: u16, quiet: bool) {
    let path = config_path(home);
    if !path.parent().is_some_and(Path::exists) {
        return;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc = if existing.trim().is_empty() {
        json!({})
    } else {
        match crate::core::jsonc::parse_jsonc(&existing) {
            Ok(v) if v.is_object() => v,
            _ => return,
        }
    };
    let ours = proxy_base_url(port);
    let mut routed = Vec::new();
    let mut changed = false;
    for (provider, key_var) in PROVIDERS {
        if !uses_api_key(home, provider, key_var) {
            changed |= remove_base_url(&mut doc, provider, &ours);
            continue;
        }
        match base_url(&doc, provider) {
            Some(url) if url == ours => routed.push(*provider),
            Some(url) => {
                if !quiet {
                    println!("  OpenCode {provider}: custom base URL {url} kept");
                }
            }
            None => {
                if !is_proxy_reachable(port) {
                    if !quiet {
                        println!(
                            "  Skipping OpenCode proxy env (proxy not running on port {port})"
                        );
                    }
                    return;
                }
                if let Some(options) = options_mut(&mut doc, provider) {
                    options.insert("baseURL".to_string(), Value::String(ours.clone()));
                    routed.push(*provider);
                    changed = true;
                }
            }
        }
    }
    if changed {
        write(&path, &doc);
    }
    if !quiet && !routed.is_empty() {
        println!(
            "  OpenCode {} through the proxy (opencode.json)",
            routed.join(" and ")
        );
    }
}

/// True when `opencode.json` carries a base URL [`install_opencode_env`]
/// wrote for `port`.
pub(crate) fn has_proxy_base_url(home: &Path, port: u16) -> bool {
    let ours = proxy_base_url(port);
    std::fs::read_to_string(config_path(home))
        .ok()
        .and_then(|s| crate::core::jsonc::parse_jsonc(&s).ok())
        .is_some_and(|doc| {
            PROVIDERS
                .iter()
                .any(|(provider, _)| base_url(&doc, provider) == Some(ours.as_str()))
        })
}

/// Removes the base URLs [`install_opencode_env`] wrote for `port`. Returns
/// whether it removed any.
pub(crate) fn uninstall_opencode_env(home: &Path, port: u16, quiet: bool) -> bool {
    let path = config_path(home);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut doc) = crate::core::jsonc::parse_jsonc(&existing) else {
        return false;
    };
    let ours = proxy_base_url(port);
    let mut changed = false;
    for (provider, _) in PROVIDERS {
        changed |= remove_base_url(&mut doc, provider, &ours);
    }
    if changed {
        write(&path, &doc);
        if !quiet {
            println!("  ✓ Removed the proxy base URLs from OpenCode's opencode.json");
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_base_url_prunes_only_what_it_emptied() {
        let ours = proxy_base_url(4444);
        let mut doc = json!({
            "mcp": {"lean-ctx": {"type": "local"}},
            "provider": {
                "anthropic": {"options": {"baseURL": ours}},
                "openai": {"options": {"baseURL": ours, "timeout": 5}},
            }
        });
        assert!(remove_base_url(&mut doc, "anthropic", &ours));
        assert!(remove_base_url(&mut doc, "openai", &ours));
        assert_eq!(
            doc,
            json!({
                "mcp": {"lean-ctx": {"type": "local"}},
                "provider": {"openai": {"options": {"timeout": 5}}}
            })
        );

        let mut custom =
            json!({"provider": {"openai": {"options": {"baseURL": "http://localhost:11434/v1"}}}});
        assert!(!remove_base_url(&mut custom, "openai", &ours));
        assert_eq!(
            base_url(&custom, "openai"),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn options_mut_creates_the_nested_objects() {
        let mut doc = json!({"$schema": "https://opencode.ai/config.json"});
        options_mut(&mut doc, "openai")
            .unwrap()
            .insert("baseURL".into(), json!("x"));
        assert_eq!(base_url(&doc, "openai"), Some("x"));
        assert_eq!(doc["$schema"], "https://opencode.ai/config.json");
    }

    #[test]
    fn stored_api_credential_routes_and_oauth_does_not() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("auth.json"),
            r#"{"anthropic":{"type":"oauth","refresh":"r"},"openai":{"type":"api","key":"sk-x"}}"#,
        )
        .unwrap();
        // The variables are unset names, so only auth.json decides.
        assert!(!uses_api_key(
            home.path(),
            "anthropic",
            "SOBRE_TEST_UNSET_A"
        ));
        assert!(uses_api_key(home.path(), "openai", "SOBRE_TEST_UNSET_B"));
    }

    #[test]
    fn install_writes_api_key_providers_and_keeps_user_urls() {
        let home = tempfile::tempdir().unwrap();
        let config = config_path(home.path());
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            r#"{"provider":{"anthropic":{"options":{"baseURL":"http://localhost:11434/v1"}}}}"#,
        )
        .unwrap();
        let auth = home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&auth).unwrap();
        std::fs::write(
            auth.join("auth.json"),
            r#"{"anthropic":{"type":"api","key":"k"},"openai":{"type":"api","key":"k"}}"#,
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        install_opencode_env(home.path(), port, true);
        assert!(has_proxy_base_url(home.path(), port));
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            base_url(&doc, "anthropic"),
            Some("http://localhost:11434/v1"),
            "a base URL the user set is kept"
        );
        assert_eq!(
            base_url(&doc, "openai"),
            Some(proxy_base_url(port).as_str())
        );

        assert!(uninstall_opencode_env(home.path(), port, true));
        assert!(!has_proxy_base_url(home.path(), port));
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(base_url(&doc, "openai"), None);
        assert_eq!(
            base_url(&doc, "anthropic"),
            Some("http://localhost:11434/v1")
        );
    }
}
