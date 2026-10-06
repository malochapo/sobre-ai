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
    /// Upstream URL per provider id, for the closed list in `PROVIDERS`
    /// plus the three built-ins (`anthropic`, `openai`, `gemini`). Only the
    /// URL is configurable; writing one is the deliberate opt-in for a custom
    /// or plaintext host. `bedrock` is served only when its URL is set.
    pub upstreams: std::collections::BTreeMap<String, String>,
    /// Model aliases and intent-tier targets, see [`RoutingRules`].
    pub routing: RoutingRules,
}

/// `[proxy.routing]` — the active router's rule set.
///
/// - **Aliases**: exact requested-model → target.
/// - **Tiers**: intent-based downgrade. The request's last user message is
///   classified (`intent_router`); the resulting tier (`fast|standard|premium`)
///   picks a target from this table. An absent tier key (or `""`) keeps the
///   requested model.
///
/// A target is `"model"` (swap the model, keep the upstream) or
/// `"provider:model"` where `provider` is a built-in (`anthropic|openai|gemini`)
/// or an id of `PROVIDERS` — then the request is also re-targeted to that
/// provider's upstream. Routing is on as soon as one rule is written.
///
/// **Fail-open by construction:** any lookup/classification/validation miss
/// routes nothing and forwards the request unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoutingRules {
    /// Exact model-name aliases: requested model → `"provider:model"` | `"model"`.
    /// BTreeMap for deterministic iteration/serialization (#498).
    pub aliases: std::collections::BTreeMap<String, String>,
    /// Intent-tier targets: `fast|standard|premium` → `"provider:model"` |
    /// `"model"` | `""` (= keep requested model).
    pub tiers: std::collections::BTreeMap<String, String>,
}

impl RoutingRules {
    /// True when the router should run at all.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !(self.aliases.is_empty() && self.tiers.is_empty())
    }
}

/// A parsed routing target: optional provider id + model name.
/// `"foundry:gpt-4o-mini"` → provider `foundry`, model `gpt-4o-mini`;
/// `"claude-haiku-4-5"` → model only (upstream unchanged).
#[must_use]
pub fn parse_route_target(target: &str) -> Option<(Option<&str>, &str)> {
    let t = target.trim();
    if t.is_empty() {
        return None;
    }
    match t.split_once(':') {
        Some((provider, model)) => {
            let (provider, model) = (provider.trim(), model.trim());
            if provider.is_empty() || model.is_empty() {
                None
            } else {
                Some((Some(provider), model))
            }
        }
        None => Some((None, t)),
    }
}

/// The API dialect an upstream endpoint speaks — deliberately separate from the
/// provider's *identity*. lean-ctx understands four wire shapes; any number of
/// configured providers (Foundry, OpenRouter, Groq, a local vLLM…) map onto
/// them. New shape = code; new provider = config (universal-provider-framework).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireShape {
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
    /// OpenAI Chat Completions / Responses API (also spoken by Azure AI
    /// Foundry, OpenRouter, Groq, vLLM, Ollama, LM Studio…).
    OpenAi,
    /// Google Gemini `generateContent` API.
    Gemini,
    /// Amazon Bedrock Runtime `InvokeModel` / event-stream API.
    Bedrock,
}

impl WireShape {
    /// Stable lowercase name (serde representation) for logs and `/status`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WireShape::Anthropic => "anthropic",
            WireShape::OpenAi => "openai",
            WireShape::Gemini => "gemini",
            WireShape::Bedrock => "bedrock",
        }
    }
}

/// One provider the proxy serves under `/providers/{id}/...`. The list is
/// closed: a new provider is a code change, only its URL is configuration.
struct ProviderSpec {
    id: &'static str,
    shape: WireShape,
    /// `None` = served only when `[proxy.upstreams]` sets a URL.
    default_url: Option<&'static str>,
}

/// The providers served under `/providers/{id}/...`, besides the three
/// built-in routes (`anthropic`, `openai`, `gemini`).
const PROVIDERS: &[ProviderSpec] = &[
    // Local inference; billed at the local shadow rate.
    ProviderSpec {
        id: "ollama",
        shape: WireShape::OpenAi,
        default_url: Some("http://127.0.0.1:11434"),
    },
    // Amazon Bedrock Runtime, SigV4 with the standard AWS credential
    // variables. The region is read from the endpoint host.
    ProviderSpec {
        id: "bedrock",
        shape: WireShape::Bedrock,
        default_url: None,
    },
];

/// The built-in provider routes (`/v1/...`), configurable in
/// `[proxy.upstreams]` alongside `PROVIDERS`.
const BUILTIN_IDS: &[&str] = &["anthropic", "openai", "gemini"];

/// Every id `[proxy.upstreams]` accepts, built-ins first.
pub fn upstream_ids() -> impl Iterator<Item = &'static str> {
    BUILTIN_IDS
        .iter()
        .copied()
        .chain(PROVIDERS.iter().map(|p| p.id))
}

/// A validated, ready-to-serve provider (published inside [`Upstreams`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub id: String,
    pub shape: WireShape,
    pub base_url: String,
    /// Validated SigV4 region for Bedrock; absent for other shapes.
    pub aws_region: Option<String>,
    /// Billed as local inference (shadow rate): the URL is loopback.
    pub local: bool,
}

fn valid_aws_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The AWS region named by a Bedrock Runtime endpoint
/// (`bedrock-runtime[-fips].{region}.amazonaws.com`, `.api.aws`, `.com.cn`,
/// or a `*.bedrock-runtime.{region}.vpce.amazonaws.com` interface endpoint).
fn bedrock_region(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let at = labels
        .iter()
        .position(|l| *l == "bedrock-runtime" || *l == "bedrock-runtime-fips")?;
    let region = labels.get(at + 1)?;
    let tail = labels[at + 2..].join(".");
    let known_tail = matches!(
        tail.as_str(),
        "amazonaws.com" | "amazonaws.com.cn" | "api.aws" | "vpce.amazonaws.com"
    );
    (known_tail && valid_aws_region(region)).then(|| (*region).to_string())
}

impl ResolvedProvider {
    /// Whether the proxy signs the request itself instead of forwarding the
    /// caller's credentials: Bedrock (SigV4).
    #[must_use]
    pub fn injects_gateway_credential(&self) -> bool {
        self.shape == WireShape::Bedrock
    }
}

impl ProxyConfig {
    /// Per-tool live-compress exclusion patterns (#481): protect Serena.
    #[must_use]
    pub fn live_compress_exclude_patterns(&self) -> Vec<String> {
        default_live_compress_exclude()
    }

    /// Whether `tool_name` is on the live-compress exclusion list (#481) and must
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

    /// `(configured value, provider default)` for one built-in provider.
    fn provider_spec(&self, provider: ProxyProvider) -> (Option<String>, &'static str) {
        match provider {
            ProxyProvider::Anthropic => (
                self.configured_upstream("anthropic"),
                "https://api.anthropic.com",
            ),
            ProxyProvider::OpenAi => (self.configured_upstream("openai"), "https://api.openai.com"),
            ProxyProvider::Gemini => (
                self.configured_upstream("gemini"),
                "https://generativelanguage.googleapis.com",
            ),
        }
    }

    /// Resolve one built-in upstream: `[proxy.upstreams]` > provider default.
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

    /// Effective upstream for a built-in provider (config > default). An
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
            gemini: self.resolve_upstream(ProxyProvider::Gemini),
            providers: self.resolve_providers(),
        }
    }

    /// Resolve the providers of `PROVIDERS`. An invalid URL is logged and the
    /// provider skipped: one typo must never take the proxy down.
    #[must_use]
    pub fn resolve_providers(&self) -> Vec<ResolvedProvider> {
        let mut out = Vec::new();
        for spec in PROVIDERS {
            let Some(url) = self
                .configured_upstream(spec.id)
                .or_else(|| spec.default_url.map(normalize_url))
            else {
                continue;
            };
            let base_url = match validate_upstream_url(&url) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!("[proxy.upstreams] '{}' is invalid — skipped: {e}", spec.id);
                    continue;
                }
            };
            let aws_region = if spec.shape == WireShape::Bedrock {
                let Some(region) = bedrock_region(&base_url) else {
                    tracing::warn!(
                        "[proxy.upstreams] '{}' is not a Bedrock Runtime endpoint — skipped",
                        spec.id
                    );
                    continue;
                };
                Some(region)
            } else {
                None
            };
            out.push(ResolvedProvider {
                id: spec.id.to_string(),
                shape: spec.shape,
                local: is_local_proxy_url(&base_url),
                base_url,
                aws_region,
            });
        }
        out
    }

    /// Re-resolve upstreams for a *running* proxy (#449). For any built-in whose
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
            gemini: keep(ProxyProvider::Gemini, &last.gemini),
            providers: self.resolve_providers(),
        }
    }
}

/// The resolved provider upstreams a running proxy forwards to. Published
/// to request handlers via a `tokio::sync::watch` channel so a config change is
/// picked up live, without a proxy restart (#449).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstreams {
    pub anthropic: String,
    pub openai: String,
    pub gemini: String,
    /// The providers of `PROVIDERS`, validated and live-reloadable exactly
    /// like the built-ins.
    pub providers: Vec<ResolvedProvider>,
}

impl Upstreams {
    /// Look up a registry provider by id (`/providers/{id}/...` route, router
    /// upstream overrides). Built-ins are not addressed here.
    #[must_use]
    pub fn provider_by_id(&self, id: &str) -> Option<&ResolvedProvider> {
        self.providers.iter().find(|p| p.id == id)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ProxyProvider {
    Anthropic,
    OpenAi,
    Gemini,
}

/// True when a running proxy's live upstream differs from what config.toml
/// resolves to. The proxy keeps its last good value when a config edit fails
/// validation, so this flags an invalid upstream that never took effect.
#[must_use]
pub fn upstream_drifted(configured: &str, live: &str) -> bool {
    configured != live
}

/// Built-in default live-compress exclusion (#481). Serena's code-reading tools
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
