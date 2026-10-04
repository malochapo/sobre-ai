//! Advanced config sections (proxy, memory subsystems, setup, llm).
//! Split out of `schema/mod.rs`; `use super::*` re-imports helpers + `SectionSchema`.

#[allow(clippy::wildcard_imports)]
use super::*;
use std::collections::BTreeMap;

pub(super) fn build(sections: &mut BTreeMap<String, SectionSchema>) {
    let cfg = crate::core::config::Config::default();
    let mut proxy = BTreeMap::new();
    proxy.insert(
        "anthropic_upstream".into(),
        key(
            "string?",
            serde_json::json!(cfg.proxy.anthropic_upstream),
            "Custom upstream URL for Anthropic API proxy",
        ),
    );
    proxy.insert(
        "openai_upstream".into(),
        key(
            "string?",
            serde_json::json!(cfg.proxy.openai_upstream),
            "Custom upstream URL for OpenAI API proxy",
        ),
    );
    proxy.insert(
        "gemini_upstream".into(),
        key(
            "string?",
            serde_json::json!(cfg.proxy.gemini_upstream),
            "Custom upstream URL for Gemini API proxy",
        ),
    );
    proxy.insert(
        "allow_insecure_http_upstream".into(),
        key(
            "bool",
            serde_json::json!(cfg.proxy.allow_insecure_http_upstream.unwrap_or(false)),
            "Allow a non-loopback plaintext http:// upstream (trusted local network only, e.g. http://host.docker.internal:2455 in front of codex-lb). Opt-in; default false",
        ),
    );
    proxy.insert(
        "allow_custom_upstream".into(),
        key(
            "bool",
            serde_json::json!(cfg.proxy.allow_custom_upstream.unwrap_or(false)),
            "Allow a custom (non-allowlisted) HTTPS upstream host, e.g. a corporate gateway in front of the provider API. Opt-in; default false. Unlike the env var, this config flag reaches the managed (service-spawned) proxy started by `proxy enable`/`restart` (#590)",
        ),
    );
    sections.insert(
        "proxy".into(),
        SectionSchema {
            description: "Proxy upstream configuration for API routing".into(),
            keys: proxy,
        },
    );

    let mut setup_keys = BTreeMap::new();
    setup_keys.insert(
            "auto_inject_rules".into(),
            key(
                "bool?",
                serde_json::json!(null),
                "Inject agent rule files during setup/update. null=auto (inject if already present), true=always, false=never",
            ),
        );
    setup_keys.insert(
        "auto_update_mcp".into(),
        key(
            "bool",
            serde_json::json!(true),
            "Register lean-ctx MCP server in editor configs during setup/update",
        ),
    );
    sections.insert(
            "setup".into(),
            SectionSchema {
                description: "Controls what lean-ctx injects during setup and updates. Fresh installs default to non-invasive (rules/skills off, MCP on).".into(),
                keys: setup_keys,
            },
        );
}
