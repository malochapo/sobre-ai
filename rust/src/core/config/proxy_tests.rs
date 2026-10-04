//! Unit tests for the proxy config (split from `proxy.rs` — LOC budget).

use super::*;

#[test]
fn loopback_http_is_always_allowed() {
    assert_eq!(
        validate_upstream_url("http://127.0.0.1:4444", false, false).unwrap(),
        "http://127.0.0.1:4444"
    );
    assert_eq!(
        validate_upstream_url("http://localhost:2455/", false, false).unwrap(),
        "http://localhost:2455"
    );
}

#[test]
fn https_allowlisted_host_is_allowed() {
    assert_eq!(
        validate_upstream_url("https://api.openai.com", false, false).unwrap(),
        "https://api.openai.com"
    );
}

#[test]
fn non_loopback_http_is_rejected_without_optin() {
    let err = validate_upstream_url("http://host.docker.internal:2455", false, false).unwrap_err();
    // The hint must point at the flag that actually lifts the scheme check
    // (#440). The custom-host opt-in never bypassed the HTTPS requirement.
    assert!(
        err.contains("allow_insecure_http_upstream"),
        "hint must name the working opt-in, got: {err}"
    );
}

#[test]
fn non_loopback_http_is_allowed_with_optin() {
    assert_eq!(
        validate_upstream_url("http://host.docker.internal:2455", true, false).unwrap(),
        "http://host.docker.internal:2455"
    );
}

#[test]
fn unknown_scheme_is_rejected() {
    assert!(validate_upstream_url("ftp://example.com", true, true).is_err());
}

#[test]
fn https_custom_host_is_rejected_without_optin() {
    // #590: a custom HTTPS host (e.g. a corporate gateway) is blocked unless
    // the operator opts in. The hint must name the config flag.
    let err = validate_upstream_url("https://gw.corp.example/anthropic", false, false).unwrap_err();
    assert!(
        err.contains("allow_custom_upstream"),
        "hint must name the opt-in, got: {err}"
    );
}

#[test]
fn https_custom_host_is_allowed_with_optin() {
    // The opt-in (`[proxy] allow_custom_upstream`) lifts the allowlist.
    assert_eq!(
        validate_upstream_url("https://gw.corp.example/anthropic", false, true).unwrap(),
        "https://gw.corp.example/anthropic"
    );
}

#[test]
fn config_flag_enables_custom_upstream_optin() {
    // #590: mirrors `config_flag_enables_insecure_http_optin`. `Some(true)`
    // resolves to true regardless of the environment, so no env mutation.
    let cfg = ProxyConfig {
        allow_custom_upstream: Some(true),
        ..Default::default()
    };
    assert!(cfg.allows_custom_upstream());
}

#[test]
fn has_custom_host_upstream_detects_only_custom_https() {
    // A custom HTTPS host counts; an allowlisted host, a loopback URL, and an
    // unset upstream do not (the http case is the insecure-http opt-in's job).
    assert!(
        ProxyConfig {
            anthropic_upstream: Some("https://gw.corp.example/anthropic".into()),
            ..Default::default()
        }
        .has_custom_host_upstream()
    );
    assert!(
        !ProxyConfig {
            openai_upstream: Some("https://api.openai.com".into()),
            anthropic_upstream: Some("http://127.0.0.1:4444".into()),
            ..Default::default()
        }
        .has_custom_host_upstream()
    );
    assert!(!ProxyConfig::default().has_custom_host_upstream());
}

#[test]
fn cold_prefix_repack_is_off() {
    // #480: off (a wrong cold guess re-bills reads as writes ~12x).
    assert!(
        !ProxyConfig::default().repacks_cold_prefix(),
        "cold-prefix repack must be off"
    );
}

#[test]
fn ccr_inband_is_off() {
    // #493: off (the splice mutates provider-visible content for the expand
    // turn).
    assert!(
        !ProxyConfig::default().ccr_inband_enabled(),
        "in-band CCR must be off"
    );
}

#[test]
fn cache_breakpoint_is_off() {
    // #939: off (it reshapes the provider-visible system field).
    assert!(
        !ProxyConfig::default().cache_breakpoint_enabled(),
        "cache-breakpoint injection must be off"
    );
}

#[test]
fn cache_aligner_is_on() {
    // #986: the volatile-field scan is measurement-only and strictly
    // cache-safe, so it is on.
    assert!(
        ProxyConfig::default().cache_aligner_enabled(),
        "cache-aligner telemetry must be on (measurement-only, safe)"
    );
}

#[test]
fn cache_align_relocate_is_off() {
    // #974: off (it reshapes the provider-visible system field by relocating
    // volatile values to the tail).
    assert!(
        !ProxyConfig::default().cache_align_relocate_enabled(),
        "active cache-aligner relocate must be off"
    );
}

#[test]
fn cache_policy_is_on() {
    // #986: telemetry + a more-conservative repack gate are both strictly
    // safe, so cache-economics is on.
    assert!(
        ProxyConfig::default().cache_policy_enabled(),
        "cache-economics must be on (measurement + safe gate)"
    );
}

#[test]
fn effort_is_off() {
    // #834: effort control is off (a strict no-op).
    assert_eq!(
        ProxyConfig::default().resolved_effort(),
        None,
        "effort control must be off"
    );
}

#[test]
fn prose_ranker_is_auto() {
    // #895: the premium extractive path (with truncation fallback) is used.
    assert_eq!(
        ProxyConfig::default().resolved_prose_ranker(),
        ProseRanker::Auto
    );
}

#[test]
fn output_holdout_is_off() {
    assert_eq!(ProxyConfig::default().output_holdout_fraction(), 0.0);
}

#[test]
fn verbosity_steer_is_off() {
    assert!(!ProxyConfig::default().verbosity_steer_enabled());
}

#[test]
fn config_flag_enables_insecure_http_optin() {
    // `Some(true)` resolves to `true` regardless of the environment, so this
    // assertion is robust without mutating process-global env vars.
    let cfg = ProxyConfig {
        allow_insecure_http_upstream: Some(true),
        ..Default::default()
    };
    assert!(cfg.allows_insecure_http_upstream());
}

/// Loopback HTTP is an always-valid custom upstream (no allowlist / opt-in
/// required).
#[test]
fn resolve_all_uses_config_then_default() {
    let cfg = ProxyConfig {
        openai_upstream: Some("http://127.0.0.1:19101".into()),
        ..Default::default()
    };
    let up = cfg.resolve_all();
    assert_eq!(up.openai, "http://127.0.0.1:19101");
    assert_eq!(up.anthropic, "https://api.anthropic.com");
    assert_eq!(up.gemini, "https://generativelanguage.googleapis.com");
}

#[test]
fn resolve_all_honors_custom_upstream_via_config_flag() {
    // #590: with the config opt-in, a custom HTTPS host resolves; without it,
    // it falls back to the provider default. This is the regression guard for
    // the reported bug.
    let custom = ProxyConfig {
        anthropic_upstream: Some("https://gw.corp.example/anthropic".into()),
        allow_custom_upstream: Some(true),
        ..Default::default()
    };
    assert_eq!(
        custom.resolve_all().anthropic,
        "https://gw.corp.example/anthropic",
        "config flag must let the managed proxy honor the custom upstream"
    );

    let blocked = ProxyConfig {
        anthropic_upstream: Some("https://gw.corp.example/anthropic".into()),
        ..Default::default()
    };
    assert_eq!(
        blocked.resolve_all().anthropic,
        "https://api.anthropic.com",
        "without the opt-in the custom host is rejected → provider default"
    );
}

#[test]
fn resolve_all_normalizes_trailing_slash() {
    let cfg = ProxyConfig {
        openai_upstream: Some("http://127.0.0.1:19101/".into()),
        ..Default::default()
    };
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
    let cfg = ProxyConfig {
        openai_upstream: Some("not-a-valid-url".into()),
        ..Default::default()
    };
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
    let cfg = ProxyConfig {
        openai_upstream: Some("http://127.0.0.1:19102".into()),
        ..Default::default()
    };
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

fn entry(id: &str, shape: WireShape, base_url: &str) -> ProviderEntry {
    ProviderEntry {
        id: id.into(),
        shape,
        base_url: base_url.into(),
        api_key_env: None,
        aws_region: None,
        enabled: None,
        local: None,
    }
}

#[test]
fn provider_local_flag_explicit_beats_url_derivation() {
    // Loopback URL → derived local; explicit flag wins in both directions
    // (host.docker.internal is the containerized-gateway case, #15/#18).
    let loopback = entry("ollama", WireShape::OpenAi, "http://127.0.0.1:11434");
    let mut declared_local = entry("hostgw", WireShape::OpenAi, "https://ollama.corp.example");
    declared_local.local = Some(true);
    let mut declared_cloud = entry("tunnel", WireShape::OpenAi, "http://localhost:9999");
    declared_cloud.local = Some(false);
    let cfg = ProxyConfig {
        providers: vec![loopback, declared_local, declared_cloud],
        ..Default::default()
    };
    let resolved = cfg.resolve_providers();
    assert_eq!(resolved.len(), 3);
    assert!(resolved[0].local, "loopback URL derives local=true");
    assert!(
        resolved[1].local,
        "explicit local=true wins over HTTPS host"
    );
    assert!(
        !resolved[2].local,
        "explicit local=false wins over loopback"
    );
}

#[test]
fn provider_registry_resolves_valid_entries() {
    // enterprise#7: a new OpenAI-compatible provider is pure config. A
    // declared HTTPS entry is its own custom-host opt-in (no extra flag).
    let cfg = ProxyConfig {
        providers: vec![
            entry(
                "foundry",
                WireShape::OpenAi,
                "https://acme.services.ai.azure.com/",
            ),
            entry("local-vllm", WireShape::OpenAi, "http://127.0.0.1:8000"),
        ],
        ..Default::default()
    };
    let resolved = cfg.resolve_providers();
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].id, "foundry");
    assert_eq!(resolved[0].shape, WireShape::OpenAi);
    assert_eq!(
        resolved[0].base_url, "https://acme.services.ai.azure.com",
        "base_url must be normalized (trailing slash stripped)"
    );
    assert_eq!(resolved[1].base_url, "http://127.0.0.1:8000");
}

#[test]
fn bedrock_provider_requires_region_matching_endpoint_and_aws_env_mode() {
    let mut valid = entry(
        "bedrock",
        WireShape::Bedrock,
        "https://bedrock-runtime.us-east-1.amazonaws.com",
    );
    valid.aws_region = Some("us-east-1".into());
    let mut wrong_region = valid.clone();
    wrong_region.id = "wrong-region".into();
    wrong_region.aws_region = Some("eu-west-1".into());
    let mut keyed = valid.clone();
    keyed.id = "caller-key".into();
    keyed.api_key_env = Some("AWS_ACCESS_KEY_ID".into());
    let resolved = ProxyConfig {
        providers: vec![valid, wrong_region, keyed],
        ..Default::default()
    }
    .resolve_providers();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].shape, WireShape::Bedrock);
    assert_eq!(resolved[0].aws_region.as_deref(), Some("us-east-1"));
    assert!(resolved[0].injects_gateway_credential());
}

#[test]
fn provider_registry_skips_invalid_entries_without_killing_rest() {
    let cfg = ProxyConfig {
        providers: vec![
            // Shadows a built-in name → skipped.
            entry("openai", WireShape::OpenAi, "https://evil.example"),
            // Uppercase/slash ids are unusable as a path segment → skipped.
            entry("Bad/Id", WireShape::OpenAi, "https://ok.example"),
            // Non-loopback plaintext HTTP without the opt-in → skipped.
            entry("insecure", WireShape::OpenAi, "http://gw.corp.example"),
            // Duplicate id → first wins.
            entry("groq", WireShape::OpenAi, "https://api.groq.com"),
            entry("groq", WireShape::OpenAi, "https://other.example"),
        ],
        ..Default::default()
    };
    let resolved = cfg.resolve_providers();
    assert_eq!(resolved.len(), 1, "only the first 'groq' entry survives");
    assert_eq!(resolved[0].id, "groq");
    assert_eq!(resolved[0].base_url, "https://api.groq.com");
}

#[test]
fn provider_registry_respects_enabled_flag_and_trims_key_env() {
    let mut disabled = entry("foundry", WireShape::OpenAi, "https://f.example");
    disabled.enabled = Some(false);
    let mut keyed = entry("router", WireShape::Anthropic, "https://r.example");
    keyed.api_key_env = Some("  ROUTER_KEY  ".into());
    let cfg = ProxyConfig {
        providers: vec![disabled, keyed],
        ..Default::default()
    };
    let resolved = cfg.resolve_providers();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].api_key_env.as_deref(), Some("ROUTER_KEY"));
    assert_eq!(
        Upstreams {
            anthropic: String::new(),
            openai: String::new(),
            gemini: String::new(),
            providers: resolved,
        }
        .provider_by_id("router")
        .map(|p| p.shape),
        Some(WireShape::Anthropic)
    );
}

#[test]
fn provider_entry_toml_round_trip() {
    // The `[[proxy.providers]]` TOML surface: shape names are lowercase,
    // api_key_env/enabled optional.
    let toml_src = r#"
        anthropic_upstream = "https://api.anthropic.com"

        [[providers]]
        id = "foundry"
        shape = "openai"
        base_url = "https://acme.services.ai.azure.com"
        api_key_env = "FOUNDRY_API_KEY"

        [[providers]]
        id = "claude-gw"
        shape = "anthropic"
        base_url = "https://gw.corp.example/anthropic"
        enabled = false
    "#;
    let cfg: ProxyConfig = toml::from_str(toml_src).expect("parse [[providers]]");
    assert_eq!(cfg.providers.len(), 2);
    assert_eq!(cfg.providers[0].shape, WireShape::OpenAi);
    assert_eq!(
        cfg.providers[0].api_key_env.as_deref(),
        Some("FOUNDRY_API_KEY")
    );
    assert_eq!(cfg.providers[1].enabled, Some(false));
    // Only the enabled entry resolves.
    assert_eq!(cfg.resolve_providers().len(), 1);
}

#[test]
fn routing_toml_round_trip() {
    // The `[proxy.routing]` TOML surface (enterprise#13). An absent table must
    // default to inactive.
    let toml_src = r#"
        [routing]
        enabled = true

        [routing.aliases]
        "acme/fast" = "foundry:gpt-4o-mini"

        [routing.tiers]
        fast = "foundry:phi-4"
        premium = ""
    "#;
    let cfg: ProxyConfig = toml::from_str(toml_src).expect("parse routing");
    assert!(cfg.routing.is_active());
    assert_eq!(
        cfg.routing.aliases.get("acme/fast").map(String::as_str),
        Some("foundry:gpt-4o-mini")
    );
    assert_eq!(
        cfg.routing.tiers.get("fast").map(String::as_str),
        Some("foundry:phi-4")
    );

    let empty: ProxyConfig = toml::from_str("").expect("empty config");
    assert!(!empty.routing.is_active(), "absent [routing] = passthrough");

    // enabled=true with no rules is still inactive (nothing to apply).
    let enabled_only: ProxyConfig = toml::from_str("[routing]\nenabled = true").expect("parse");
    assert!(!enabled_only.routing.is_active());
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
fn role_aggressiveness_is_off() {
    // A config compresses no prose, so the proxy stays byte-for-byte
    // unchanged (#710).
    let cfg = ProxyConfig::default();
    assert_eq!(cfg.resolved_role_aggressiveness(ProseRole::System), None);
    assert_eq!(cfg.resolved_role_aggressiveness(ProseRole::User), None);
}

#[test]
fn live_compress_is_on() {
    // #481: the proxy live-compresses tool results.
    assert!(
        ProxyConfig::default().live_compresses(),
        "live_compress must be on"
    );
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

#[test]
fn compress_protect_is_a_noop() {
    // #1150: nothing is protected, so compression stays on for all.
    let cfg = ProxyConfig::default();
    assert!(!cfg.is_path_compress_protected("tests/golden/output.snap"));
    assert!(cfg.compress_protect_globs().is_empty());
}
