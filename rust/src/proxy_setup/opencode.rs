//! OpenCode proxy wiring: `provider.<id>.options.baseURL` in OpenCode's
//! `opencode.json`, its own setting for a provider's base URL.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::util::is_proxy_reachable;

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

/// True when `url` has the shape of a base URL sobre wrote, whatever the
/// port: `http://127.0.0.1:<port>/v1`. A `localhost` or any other URL is
/// the user's.
fn is_proxy_base_url(url: &str) -> bool {
    url.strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix("/v1"))
        .is_some_and(|port| port.parse::<u16>().is_ok())
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
    if !base_url(doc, provider).is_some_and(is_proxy_base_url) {
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

fn read(path: &Path) -> Option<Value> {
    let existing = std::fs::read_to_string(path).ok()?;
    crate::core::jsonc::parse_jsonc(&existing).ok()
}

fn write(path: &Path, doc: &Value) {
    if let Ok(out) = serde_json::to_string_pretty(doc) {
        let _ = std::fs::write(path, out + "\n");
    }
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
            Some(url) if url == ours => routed.push(*provider),
            Some(url) if !is_proxy_base_url(url) => {
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
                    routed.push(*provider);
                    changed = true;
                }
            }
        }
    }
    if changed {
        write(&path, &doc);
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
        PROVIDERS
            .iter()
            .any(|(provider, _)| base_url(&doc, provider).is_some_and(is_proxy_base_url))
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

    fn config_path(home: &Path) -> std::path::PathBuf {
        crate::core::editor_registry::opencode_config_path(home)
    }

    #[test]
    fn own_url_is_recognised_by_shape_on_any_port() {
        assert!(is_proxy_base_url("http://127.0.0.1:4444/v1"));
        assert!(is_proxy_base_url("http://127.0.0.1:5123/v1"));
        for user in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:4444",
            "http://127.0.0.1:4444/v1/extra",
            "https://api.openai.com/v1",
            "http://127.0.0.1:notaport/v1",
        ] {
            assert!(!is_proxy_base_url(user), "{user}");
        }
    }

    #[test]
    fn remove_base_url_prunes_only_what_it_emptied() {
        let ours = proxy_base_url(4444);
        let mut doc = json!({
            "mcp": {"lean-ctx": {"type": "local"}},
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
                "mcp": {"lean-ctx": {"type": "local"}},
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

    /// `proxy.port` changed between two `proxy enable`s: the URL written for
    /// the old port is moved to the new one, and disable removes it whatever
    /// port it names.
    #[test]
    fn a_url_written_for_another_port_follows_the_proxy() {
        let home = home_with_api_keys();
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

        std::fs::write(
            config_path(home.path()),
            json!({"provider": {"openai": {"options": {"baseURL": proxy_base_url(1)}}}})
                .to_string(),
        )
        .unwrap();
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
