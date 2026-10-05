//! Advanced config sections (proxy, memory subsystems, setup, llm).
//! Split out of `schema/mod.rs`; `use super::*` re-imports helpers + `SectionSchema`.

#[allow(clippy::wildcard_imports)]
use super::*;
use std::collections::BTreeMap;

pub(super) fn build(sections: &mut BTreeMap<String, SectionSchema>) {
    let mut proxy = BTreeMap::new();
    proxy.insert(
        "enabled".into(),
        key(
            "bool?",
            serde_json::json!(null),
            "Run the local API proxy and point installed agents at it. null = not decided yet (init asks), false = never touch it",
        ),
    );
    proxy.insert(
        "port".into(),
        key(
            "u16?",
            serde_json::json!(null),
            "Port the proxy listens on (default: 4444)",
        ),
    );
    for id in crate::core::config::upstream_ids() {
        proxy.insert(
            format!("upstreams.{id}"),
            key(
                "string?",
                serde_json::json!(null),
                &format!(
                    "Upstream URL for the {id} provider (unset = its default{}). Writing a URL is the opt-in for a custom or plaintext host",
                    if id == "bedrock" { "; bedrock is served only when set" } else { "" }
                ),
            ),
        );
    }
    proxy.insert(
        "routing".into(),
        key(
            "table",
            serde_json::json!({}),
            "Model routing: [proxy.routing.aliases] maps a requested model to \"model\" or \"provider:model\"; [proxy.routing.tiers] maps fast/standard/premium to a target. Active as soon as one rule is written",
        ),
    );
    sections.insert(
        "proxy".into(),
        SectionSchema {
            description: "The local API proxy: switch, port, upstream URLs and model routing"
                .into(),
            keys: proxy,
        },
    );
}
