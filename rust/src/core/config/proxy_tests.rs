use super::*;

fn with_upstream(id: &str, url: &str) -> ProxyConfig {
    let mut cfg = ProxyConfig::default();
    cfg.upstreams.insert(id.to_string(), url.to_string());
    cfg
}

#[test]
fn any_http_or_https_url_is_accepted() {
    for url in [
        "http://127.0.0.1:8080",
        "https://api.openai.com",
        "http://host.docker.internal:2455",
        "https://gw.corp.example/anthropic",
    ] {
        assert_eq!(validate_upstream_url(url).as_deref(), Ok(url));
    }
}

#[test]
fn unknown_scheme_is_rejected() {
    assert!(validate_upstream_url("ftp://example.com").is_err());
    assert!(validate_upstream_url("not-a-valid-url").is_err());
}

#[test]
fn resolve_all_uses_config_then_default() {
    let up = with_upstream("openai", "http://127.0.0.1:19101").resolve_all();
    assert_eq!(up.openai, "http://127.0.0.1:19101");
    assert_eq!(up.anthropic, "https://api.anthropic.com");
}

#[test]
fn resolve_all_honors_custom_https_upstream() {
    let cfg = with_upstream("anthropic", "https://gw.corp.example/anthropic");
    assert_eq!(
        cfg.resolve_all().anthropic,
        "https://gw.corp.example/anthropic"
    );
}

#[test]
fn resolve_all_normalizes_trailing_slash() {
    let cfg = with_upstream("openai", "http://127.0.0.1:19101/");
    assert_eq!(cfg.resolve_all().openai, "http://127.0.0.1:19101");
}

#[test]
fn refresh_keeps_last_good_on_invalid_config() {
    // A typo in config.toml must never reroute a live proxy to the default.
    let last = Upstreams {
        anthropic: "https://api.anthropic.com".into(),
        openai: "http://127.0.0.1:19101".into(),
    };
    let cfg = with_upstream("openai", "not-a-valid-url");
    assert_eq!(
        cfg.refresh_upstreams(&last).openai,
        "http://127.0.0.1:19101",
        "invalid upstream → keep last good, never silently fall to default"
    );
}

#[test]
fn refresh_adopts_valid_config_change() {
    let last = Upstreams {
        anthropic: "https://api.anthropic.com".into(),
        openai: "http://127.0.0.1:19101".into(),
    };
    let cfg = with_upstream("openai", "http://127.0.0.1:19102");
    assert_eq!(
        cfg.refresh_upstreams(&last).openai,
        "http://127.0.0.1:19102"
    );
}

#[test]
fn upstream_drift_config_changed_needs_restart() {
    assert!(upstream_drifted(
        "http://127.0.0.1:2455",
        "https://api.openai.com"
    ));
}

#[test]
fn upstream_drift_in_sync() {
    assert!(!upstream_drifted(
        "https://api.openai.com",
        "https://api.openai.com"
    ));
}

#[test]
fn upstreams_are_anthropic_and_openai_only() {
    assert_eq!(upstream_ids().collect::<Vec<_>>(), ["anthropic", "openai"]);
}

#[test]
fn local_ollama_is_an_anthropic_upstream() {
    // Ollama speaks the Anthropic Messages API, so a Claude-shaped client
    // reaches a local model through the `anthropic` upstream.
    let cfg = with_upstream("anthropic", "http://127.0.0.1:11434");
    assert_eq!(cfg.resolve_all().anthropic, "http://127.0.0.1:11434");
}

#[test]
fn proxy_table_toml_round_trip() {
    let toml_src = r#"
        enabled = true
        port = 4545

        [upstreams]
        anthropic = "https://gw.corp.example/anthropic"
    "#;
    let cfg: ProxyConfig = toml::from_str(toml_src).expect("parse [proxy]");
    assert_eq!(cfg.enabled, Some(true));
    assert_eq!(cfg.port, Some(4545));
    assert_eq!(
        cfg.resolve_all().anthropic,
        "https://gw.corp.example/anthropic"
    );
}

#[test]
fn live_compress_exclude_defaults_to_serena() {
    // #481: an unset list protects Serena's code-reading tools, which return
    // source bodies but are mis-bucketed as `Search` by name.
    let cfg = ProxyConfig::default();
    assert!(cfg.is_tool_live_compress_excluded("mcp__serena__find_symbol"));
    assert!(cfg.is_tool_live_compress_excluded("Serena.search_for_pattern"));
    assert!(!cfg.is_tool_live_compress_excluded("sobre_shell"));
}
