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
            "Port the proxy listens on (default: 4444 + uid mod 1000 on Unix, 4444 on Windows)",
        ),
    );
    for id in crate::core::config::upstream_ids() {
        proxy.insert(
            format!("upstreams.{id}"),
            key(
                "string?",
                serde_json::json!(null),
                &format!(
                    "Upstream URL for the {id} provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host"
                ),
            ),
        );
    }
    sections.insert(
        "proxy".into(),
        SectionSchema {
            description: "The local API proxy: switch, port and upstream URLs".into(),
            keys: proxy,
        },
    );
}
