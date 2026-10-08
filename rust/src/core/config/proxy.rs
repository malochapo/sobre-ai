//! The `[proxy]` table: the local API proxy and where it forwards.

use serde::{Deserialize, Serialize};

/// The `[proxy]` table of `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    /// Run the local API proxy and point installed agents at it.
    /// `None` = never decided (init asks), `Some(false)` = never touch it.
    pub enabled: Option<bool>,
    /// Port the proxy listens on. `None` = the default port.
    pub port: Option<u16>,
    /// Upstream URL per provider (`anthropic`, `openai`). Only the URL is
    /// configurable; writing one is the deliberate opt-in for a custom or
    /// plaintext host, such as a local Ollama for `anthropic`.
    pub upstreams: std::collections::BTreeMap<String, String>,
}

/// Every id `[proxy.upstreams]` accepts.
pub fn upstream_ids() -> impl Iterator<Item = &'static str> {
    ["anthropic", "openai"].into_iter()
}

impl ProxyConfig {
    /// Per-tool live-compress exclusion patterns: protect Serena.
    #[must_use]
    pub fn live_compress_exclude_patterns(&self) -> Vec<String> {
        default_live_compress_exclude()
    }

    /// Whether `tool_name` is on the live-compress exclusion list and must
    /// therefore reach the model intact, like a protected file read. Matching is
    /// case-insensitive substring, mirroring `tool_kind::classify_tool_name`.
    #[must_use]
    pub fn is_tool_live_compress_excluded(&self, tool_name: &str) -> bool {
        let name = tool_name.to_ascii_lowercase();
        self.live_compress_exclude_patterns().iter().any(|p| {
            let p = p.trim().to_ascii_lowercase();
            !p.is_empty() && name.contains(p.as_str())
        })
    }

    /// The configured URL for `id`, normalized; `None` when unset or blank.
    fn configured_upstream(&self, id: &str) -> Option<String> {
        self.upstreams.get(id).and_then(|u| normalize_url_opt(u))
    }

    /// Warn once per resolution about `[proxy.upstreams]` keys that name no
    /// provider (a typo would otherwise be silently ignored).
    fn warn_unknown_upstream_ids(&self) {
        for id in self.upstreams.keys() {
            if !upstream_ids().any(|known| known == id) {
                tracing::warn!("[proxy.upstreams] unknown provider id '{id}' — ignored");
            }
        }
    }

    /// `(configured value, provider default)` for one provider.
    fn provider_spec(&self, provider: ProxyProvider) -> (Option<String>, &'static str) {
        match provider {
            ProxyProvider::Anthropic => (
                self.configured_upstream("anthropic"),
                "https://api.anthropic.com",
            ),
            ProxyProvider::OpenAi => (self.configured_upstream("openai"), "https://api.openai.com"),
        }
    }

    /// Resolve one upstream: `[proxy.upstreams]` > provider default.
    /// Returns `Err` when a value is *present but invalid* so a live reload can
    /// keep the last good value instead of silently rerouting to the default; an
    /// *absent* value resolves to the provider default (`Ok`).
    fn resolve_upstream_checked(&self, provider: ProxyProvider) -> Result<String, String> {
        let (config_val, default) = self.provider_spec(provider);
        match config_val {
            None => Ok(normalize_url(default)),
            Some(url) => validate_upstream_url(&url),
        }
    }

    /// Effective upstream for a provider (config > default). An
    /// invalid configured value falls back to the provider default (logged) —
    /// the safe choice at startup.
    pub fn resolve_upstream(&self, provider: ProxyProvider) -> String {
        match self.resolve_upstream_checked(provider) {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!("upstream validation failed, using default: {e}");
                normalize_url(self.provider_spec(provider).1)
            }
        }
    }

    /// Resolve every upstream at once.
    pub fn resolve_all(&self) -> Upstreams {
        self.warn_unknown_upstream_ids();
        Upstreams {
            anthropic: self.resolve_upstream(ProxyProvider::Anthropic),
            openai: self.resolve_upstream(ProxyProvider::OpenAi),
        }
    }

    /// Re-resolve upstreams for a *running* proxy. For any upstream whose
    /// currently configured value fails validation, the last good value is
    /// kept instead of rerouting live traffic to the provider default — so a typo
    /// in config.toml can never silently redirect in-flight requests.
    pub fn refresh_upstreams(&self, last: &Upstreams) -> Upstreams {
        let keep = |provider: ProxyProvider, prev: &str| {
            self.resolve_upstream_checked(provider).unwrap_or_else(|e| {
                tracing::warn!("upstream invalid, keeping {prev}: {e}");
                prev.to_string()
            })
        };
        Upstreams {
            anthropic: keep(ProxyProvider::Anthropic, &last.anthropic),
            openai: keep(ProxyProvider::OpenAi, &last.openai),
        }
    }
}

/// The resolved provider upstreams a running proxy forwards to. Published
/// to request handlers via a `tokio::sync::watch` channel so a config change is
/// picked up live, without a proxy restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstreams {
    pub anthropic: String,
    pub openai: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ProxyProvider {
    Anthropic,
    OpenAi,
}

/// True when a running proxy's live upstream differs from what config.toml
/// resolves to. The proxy keeps its last good value when a config edit fails
/// validation, so this flags an invalid upstream that never took effect.
#[must_use]
pub fn upstream_drifted(configured: &str, live: &str) -> bool {
    configured != live
}

/// Built-in default live-compress exclusion. Serena's code-reading tools
/// (`find_symbol`/`find_referencing_symbols`/`search_for_pattern`) return source
/// bodies the model edits, yet are mis-bucketed as `Search` by name, so the proxy
/// would otherwise gut them. Protect anything namespaced `serena` by default.
fn default_live_compress_exclude() -> Vec<String> {
    vec!["serena".to_string()]
}

pub fn normalize_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

pub fn normalize_url_opt(value: &str) -> Option<String> {
    let trimmed = normalize_url(value);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Validate a configured upstream URL. Any `http://` or `https://` URL is
/// accepted: writing it in `[proxy.upstreams]` is the deliberate choice.
pub(super) fn validate_upstream_url(url: &str) -> Result<String, String> {
    let normalized = normalize_url(url);
    if normalized.starts_with("http://") || normalized.starts_with("https://") {
        Ok(normalized)
    } else {
        Err(format!(
            "upstream URL must start with http:// or https://: {normalized}"
        ))
    }
}

pub fn is_local_proxy_url(value: &str) -> bool {
    let n = normalize_url(value);
    n.starts_with("http://127.0.0.1:")
        || n.starts_with("http://localhost:")
        || n.starts_with("http://[::1]:")
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
