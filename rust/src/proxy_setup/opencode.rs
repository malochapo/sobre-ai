//! OpenCode proxy wiring: `provider.<id>.options.baseURL` in OpenCode's
//! `opencode.json`, its own setting for a provider's base URL.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::util::is_proxy_reachable;
use super::wiring;

/// The OpenCode providers the proxy serves, with the environment variable that
/// carries each one's API key.
const PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
];

/// The base URL sobre writes. Both SDKs expect the `/v1` prefix in it.
fn proxy_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

/// The wiring slot of `provider`'s base URL.
fn slot(provider: &str) -> String {
    format!("opencode.{provider}")
}

/// True when `url`, `provider`'s base URL, is the one sobre wrote. A
/// `localhost` URL, another port or any other URL is the user's.
fn is_proxy_base_url(provider: &str, url: &str) -> bool {
    wiring::is_ours(&slot(provider), url)
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

/// Removes `provider.<id>.options.baseURL` when sobre wrote it, then any
/// object that leaves empty. Returns whether it removed anything.
fn remove_base_url(doc: &mut Value, provider: &str) -> bool {
    if !base_url(doc, provider).is_some_and(|url| is_proxy_base_url(provider, url)) {
        return false;
    }
    wiring::forget(&slot(provider));
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

fn read(path: &Path) -> Option<Value> {
    let existing = std::fs::read_to_string(path).ok()?;
    crate::core::jsonc::parse_jsonc(&existing).ok()
}

/// Writes `doc` back over `path`, keeping the user's formatting. `install`
/// keeps a backup of a file sobre edits for the first time; a removal does not.
fn write(path: &Path, doc: &Value, install: bool) {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let _ = if install {
        crate::config_io::write_json_edit(path, &original, doc)
    } else {
        crate::config_io::write_json_cleanup(path, &original, doc)
    };
}

/// Points each OpenCode provider that uses an API key at the proxy on `port`,
/// and takes one that no longer does off it. A base URL sobre wrote for
/// another port is moved to `port`; one the user set is never replaced.
pub(crate) fn install_opencode_env(home: &Path, port: u16, quiet: bool) {
    let path = crate::core::editor_registry::opencode_config_path(home);
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
    let mut reachable = None;
    let mut routed = Vec::new();
    let mut changed = false;
    for (provider, key_var) in PROVIDERS {
        if !uses_api_key(home, provider, key_var) {
            changed |= remove_base_url(&mut doc, provider);
            continue;
        }
        match base_url(&doc, provider) {
            Some(url) if url == ours => {
                wiring::record(&slot(provider), &ours, None);
                routed.push(*provider);
            }
            Some(url) if !is_proxy_base_url(provider, url) => {
                if !quiet {
                    println!("  OpenCode {provider}: custom base URL {url} kept");
                }
            }
            _ => {
                if !*reachable.get_or_insert_with(|| is_proxy_reachable(port)) {
                    continue;
                }
                if let Some(options) = options_mut(&mut doc, provider) {
                    options.insert("baseURL".to_string(), Value::String(ours.clone()));
                    wiring::record(&slot(provider), &ours, None);
                    routed.push(*provider);
                    changed = true;
                }
            }
        }
    }
    if changed {
        write(&path, &doc, true);
    }
    if !quiet {
        if reachable == Some(false) {
            println!("  Skipping OpenCode proxy env (proxy not running on port {port})");
        }
        if !routed.is_empty() {
            println!(
                "  OpenCode {} through the proxy (opencode.json)",
                routed.join(" and ")
            );
        }
    }
}

/// True when `opencode.json` carries a base URL [`install_opencode_env`]
/// wrote, on any port.
pub(crate) fn has_proxy_base_url(home: &Path) -> bool {
    read(&crate::core::editor_registry::opencode_config_path(home)).is_some_and(|doc| {
        PROVIDERS.iter().any(|(provider, _)| {
            base_url(&doc, provider).is_some_and(|url| is_proxy_base_url(provider, url))
        })
    })
}

/// Removes the base URLs [`install_opencode_env`] wrote, on any port.
/// Returns whether it removed any.
pub(crate) fn uninstall_opencode_env(home: &Path, quiet: bool) -> bool {
    let path = crate::core::editor_registry::opencode_config_path(home);
    let Some(mut doc) = read(&path) else {
        return false;
    };
    let mut changed = false;
    for (provider, _) in PROVIDERS {
        changed |= remove_base_url(&mut doc, provider);
    }
    if changed {
        write(&path, &doc, false);
        if !quiet {
            println!("  ✓ Removed the proxy base URLs from OpenCode's opencode.json");
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_path(home: &Path) -> std::path::PathBuf {
        crate::core::editor_registry::opencode_config_path(home)
    }

    /// The URL sobre recorded for a provider is its own, on any port; with
    /// no record, only the proxy's address on its port is.
    #[test]
    fn own_url_is_the_recorded_one_or_the_proxys_address() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let port = super::super::default_port();
        assert!(is_proxy_base_url("anthropic", &proxy_base_url(port)));
        wiring::record(&slot("openai"), &proxy_base_url(5123), None);
        assert!(is_proxy_base_url("openai", &proxy_base_url(5123)));
        for user in [
            "http://localhost:11434/v1".to_string(),
            "http://127.0.0.1:4000/v1".to_string(),
            format!("http://127.0.0.1:{port}/v1/extra"),
            "https://api.openai.com/v1".to_string(),
        ] {
            assert!(!is_proxy_base_url("openai", &user), "{user}");
        }
    }

    #[test]
    fn remove_base_url_prunes_only_what_it_emptied() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let ours = proxy_base_url(4444);
        wiring::record(&slot("anthropic"), &proxy_base_url(5555), None);
        wiring::record(&slot("openai"), &ours, None);
        let mut doc = json!({
            "mcp": {"sobre": {"type": "local"}},
            "provider": {
                "anthropic": {"options": {"baseURL": proxy_base_url(5555)}},
                "openai": {"options": {"baseURL": ours, "timeout": 5}},
            }
        });
        assert!(remove_base_url(&mut doc, "anthropic"));
        assert!(remove_base_url(&mut doc, "openai"));
        assert_eq!(
            doc,
            json!({
                "mcp": {"sobre": {"type": "local"}},
                "provider": {"openai": {"options": {"timeout": 5}}}
            })
        );

        let mut custom =
            json!({"provider": {"openai": {"options": {"baseURL": "http://localhost:11434/v1"}}}});
        assert!(!remove_base_url(&mut custom, "openai"));
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

    fn home_with_api_keys() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let auth = home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&auth).unwrap();
        std::fs::write(
            auth.join("auth.json"),
            r#"{"anthropic":{"type":"api","key":"k"},"openai":{"type":"api","key":"k"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(config_path(home.path()).parent().unwrap()).unwrap();
        home
    }

    fn read_doc(home: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(config_path(home)).unwrap()).unwrap()
    }

    #[test]
    fn install_writes_api_key_providers_and_keeps_user_urls() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let home = home_with_api_keys();
        let config = config_path(home.path());
        std::fs::write(
            &config,
            r#"{"provider":{"anthropic":{"options":{"baseURL":"http://localhost:11434/v1"}}}}"#,
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        install_opencode_env(home.path(), port, true);
        assert!(has_proxy_base_url(home.path()));
        let doc = read_doc(home.path());
        assert_eq!(
            base_url(&doc, "anthropic"),
            Some("http://localhost:11434/v1"),
            "a base URL the user set is kept"
        );
        assert_eq!(
            base_url(&doc, "openai"),
            Some(proxy_base_url(port).as_str())
        );

        assert!(uninstall_opencode_env(home.path(), true));
        assert!(!has_proxy_base_url(home.path()));
        let doc = read_doc(home.path());
        assert_eq!(base_url(&doc, "openai"), None);
        assert_eq!(
            base_url(&doc, "anthropic"),
            Some("http://localhost:11434/v1")
        );
    }

    /// `proxy.port` changed between two `proxy enable`s: the URL sobre wrote
    /// for the old port is moved to the new one, and disable removes it.
    #[test]
    fn a_url_written_for_another_port_follows_the_proxy() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let home = home_with_api_keys();
        for provider in ["anthropic", "openai"] {
            wiring::record(&slot(provider), &proxy_base_url(1), None);
        }
        std::fs::write(
            config_path(home.path()),
            json!({"provider": {
                "anthropic": {"options": {"baseURL": proxy_base_url(1)}},
                "openai": {"options": {"baseURL": proxy_base_url(1)}},
            }})
            .to_string(),
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        install_opencode_env(home.path(), port, true);
        let doc = read_doc(home.path());
        for provider in ["anthropic", "openai"] {
            assert_eq!(
                base_url(&doc, provider),
                Some(proxy_base_url(port).as_str())
            );
        }

        assert!(has_proxy_base_url(home.path()));
        assert!(uninstall_opencode_env(home.path(), true));
        assert_eq!(read_doc(home.path()), json!({}));
    }

    /// With the proxy down, a provider that stopped using an API key is still
    /// taken off it.
    #[test]
    fn unreachable_proxy_still_saves_a_removal() {
        // A set ANTHROPIC_API_KEY would put anthropic on a key, which this
        // scenario needs off one.
        if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
            return;
        }
        let _iso = crate::core::data_dir::isolated_data_dir();
        wiring::record(&slot("anthropic"), &proxy_base_url(1), None);
        let home = tempfile::tempdir().unwrap();
        let auth = home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&auth).unwrap();
        // anthropic logged in with OAuth now; openai still on a key.
        std::fs::write(
            auth.join("auth.json"),
            r#"{"anthropic":{"type":"oauth","refresh":"r"},"openai":{"type":"api","key":"k"}}"#,
        )
        .unwrap();
        let config = config_path(home.path());
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            json!({"provider": {"anthropic": {"options": {"baseURL": proxy_base_url(1)}}}})
                .to_string(),
        )
        .unwrap();
        // Nothing listens on this port once the listener is dropped.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };

        install_opencode_env(home.path(), port, true);
        assert_eq!(read_doc(home.path()), json!({}));
    }
}
