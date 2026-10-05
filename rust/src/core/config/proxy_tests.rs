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
    assert_eq!(up.gemini, "https://generativelanguage.googleapis.com");
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
        gemini: "https://generativelanguage.googleapis.com".into(),
        providers: Vec::new(),
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
        gemini: "https://generativelanguage.googleapis.com".into(),
        providers: Vec::new(),
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
fn default_providers_are_the_closed_list_without_bedrock() {
    let ids: Vec<String> = ProxyConfig::default()
        .resolve_providers()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, ["xai", "grok-chat", "commandcode", "ollama"]);
}

#[test]
fn ollama_is_local_and_cloud_providers_are_not() {
    let providers = ProxyConfig::default().resolve_providers();
    let local = |id: &str| providers.iter().find(|p| p.id == id).unwrap().local;
    assert!(local("ollama"));
    assert!(!local("xai"));
}

#[test]
fn upstreams_override_a_provider_url() {
    let cfg = with_upstream("ollama", "http://127.0.0.1:11500/");
    let p = cfg
        .resolve_providers()
        .into_iter()
        .find(|p| p.id == "ollama")
        .unwrap();
    assert_eq!(p.base_url, "http://127.0.0.1:11500");
    assert_eq!(p.shape, WireShape::OpenAi);
}

#[test]
fn invalid_provider_url_skips_only_that_provider() {
    let ids: Vec<String> = with_upstream("xai", "not-a-url")
        .resolve_providers()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, ["grok-chat", "commandcode", "ollama"]);
}

#[test]
fn bedrock_is_served_when_set_with_region_from_host() {
    let cfg = with_upstream("bedrock", "https://bedrock-runtime.eu-west-3.amazonaws.com");
    let p = cfg
        .resolve_providers()
        .into_iter()
        .find(|p| p.id == "bedrock")
        .expect("bedrock served once its URL is set");
    assert_eq!(p.shape, WireShape::Bedrock);
    assert_eq!(p.aws_region.as_deref(), Some("eu-west-3"));
    assert!(p.injects_gateway_credential());
}

#[test]
fn bedrock_region_reads_known_endpoint_forms() {
    for (url, region) in [
        (
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            Some("us-east-1"),
        ),
        (
            "https://bedrock-runtime-fips.us-west-2.api.aws",
            Some("us-west-2"),
        ),
        (
            "https://bedrock-runtime.cn-north-1.amazonaws.com.cn",
            Some("cn-north-1"),
        ),
        (
            "https://vpce-1.bedrock-runtime.eu-central-1.vpce.amazonaws.com",
            Some("eu-central-1"),
        ),
        ("https://bedrock-runtime.us-east-1.evil.example", None),
        ("https://bedrock-runtime.us-east-1.amazonaws.com/path", None),
        ("http://127.0.0.1:9000", None),
    ] {
        assert_eq!(bedrock_region(url).as_deref(), region, "{url}");
    }
}

#[test]
fn proxy_table_toml_round_trip() {
    let toml_src = r#"
        enabled = true
        port = 4545

        [upstreams]
        anthropic = "https://gw.corp.example/anthropic"
        ollama = "http://127.0.0.1:11500"

        [routing.aliases]
        "acme/fast" = "ollama:qwen3"

        [routing.tiers]
        fast = "ollama:qwen3"
        premium = ""
    "#;
    let cfg: ProxyConfig = toml::from_str(toml_src).expect("parse [proxy]");
    assert_eq!(cfg.enabled, Some(true));
    assert_eq!(cfg.port, Some(4545));
    assert_eq!(
        cfg.resolve_all().anthropic,
        "https://gw.corp.example/anthropic"
    );
    assert!(cfg.routing.is_active());
    assert_eq!(
        cfg.routing.aliases.get("acme/fast").map(String::as_str),
        Some("ollama:qwen3")
    );

    let empty: ProxyConfig = toml::from_str("").expect("empty config");
    assert!(!empty.routing.is_active(), "absent [routing] = passthrough");
}

#[test]
fn parse_route_target_shapes() {
    assert_eq!(
        parse_route_target("foundry:gpt-4o-mini"),
        Some((Some("foundry"), "gpt-4o-mini"))
    );
    assert_eq!(
        parse_route_target(" claude-haiku-4-5 "),
        Some((None, "claude-haiku-4-5"))
    );
    assert_eq!(parse_route_target(""), None);
    assert_eq!(parse_route_target("  "), None);
    assert_eq!(parse_route_target(":model"), None);
    assert_eq!(parse_route_target("provider:"), None);
}

#[test]
fn live_compress_exclude_defaults_to_serena() {
    // #481: an unset list protects Serena's code-reading tools, which return
    // source bodies but are mis-bucketed as `Search` by name.
    let cfg = ProxyConfig::default();
    assert!(cfg.is_tool_live_compress_excluded("mcp__serena__find_symbol"));
    assert!(cfg.is_tool_live_compress_excluded("Serena.search_for_pattern"));
    assert!(!cfg.is_tool_live_compress_excluded("ctx_shell"));
}
