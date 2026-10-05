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
    /// Upstream URL per provider id, for the closed list in [`PROVIDERS`]
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
/// or an id of [`PROVIDERS`] — then the request is also re-targeted to that
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
    // xAI Grok, API-key rail (`XAI_API_KEY`).
    ProviderSpec {
        id: "xai",
        shape: WireShape::OpenAi,
        default_url: Some("https://api.x.ai"),
    },
    // xAI Grok, subscription rail (`grok login` session).
    ProviderSpec {
        id: "grok-chat",
        shape: WireShape::OpenAi,
        default_url: Some("https://cli-chat-proxy.grok.com"),
    },
    ProviderSpec {
        id: "commandcode",
        shape: WireShape::OpenAi,
        default_url: Some("https://api.commandcode.ai"),
    },
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
/// `[proxy.upstreams]` alongside [`PROVIDERS`].
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

/// The conversation roles whose prose the proxy may compress in the frozen
/// region. Deliberately excludes `assistant` — model turns are never rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProseRole {
    System,
    User,
}

/// Unified proxy operation mode that sets cache-optimal defaults for all knobs.
///
/// Instead of configuring 8+ individual booleans, operators pick a single mode
/// that resolves sensible defaults. Explicit per-knob overrides always win.
///
/// - `Cache` (default): maximise provider prompt-cache hit rate. History is
///   frozen at staircase boundaries, breakpoints are injected, volatile fields
///   are detected, and the live tail is compressed — but the prefix is never
///   rewritten.
/// - `Token`: maximise raw token reduction. History may be rewritten, cold
///   prefixes repacked, and volatile fields relocated. Best for short one-shot
///   requests where cache reuse is unlikely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    Cache,
    Token,
}

impl ProxyMode {}

/// How the proxy squeezes prose it must shrink (#895).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProseRanker {
    /// Extractive embedding ranking when the engine is available, else truncate.
    /// The default — strictly better than truncation, and cache-safe via the
    /// per-content memo in [`crate::proxy::prose_ranker`].
    Auto,
    /// Same engine path as `Auto` (kept distinct so an operator can express
    /// intent / so a future "require engine" semantic has a name).
    Extractive,
    /// Original deterministic FIFO squeeze; never touches the embedding engine.
    Truncate,
}

/// How the proxy prunes old tool results from conversation history.
///
/// Provider prompt caches (Anthropic `cache_control`, OpenAI automatic prompt
/// caching) bill cached prefix tokens at a fraction of the base rate but only
/// match *exact* prefixes. Any mutation whose position depends on the current
/// conversation length (a rolling window) rewrites a previously-stable message
/// every turn, invalidating the cache from that point — turning cheap cache
/// reads into full-price writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryMode {
    /// Prune only at frozen generation boundaries that advance in large,
    /// deterministic steps. Between jumps the request prefix is byte-stable,
    /// so provider prompt caches keep hitting. Content the client has marked
    /// with a `cache_control` breakpoint is never rewritten, so an advancing
    /// boundary can no longer invalidate the already-cached prefix (#448).
    /// Default.
    CacheAware,
    /// Legacy behaviour: summarize everything older than the last N messages.
    /// Maximum raw-token reduction, but defeats provider prompt caching.
    Rolling,
    /// Never prune history (tool-result compression still applies — it is
    /// content-deterministic and therefore prefix-stable).
    Off,
}

impl ProxyConfig {
    /// The proxy mode: [`ProxyMode::Cache`], which keeps the request prefix
    /// byte-stable for provider prompt caches.
    #[must_use]
    pub fn resolved_proxy_mode(&self) -> ProxyMode {
        ProxyMode::Cache
    }

    /// Whether a Headroom-compatible stack is configured. No.
    #[must_use]
    pub fn is_headroom_compat(&self) -> bool {
        false
    }

    /// History pruning: cache-aware, so provider prompt caches keep hitting.
    pub fn resolved_history_mode(&self) -> HistoryMode {
        HistoryMode::CacheAware
    }

    /// Whether the proxy injects `stream_options.include_usage` into streamed
    /// OpenAI Chat Completions to meter real spend. Yes.
    pub fn meters_openai_usage(&self) -> bool {
        true
    }

    /// Operator-configured extra cost header (#1189). None: LiteLLM's standard
    /// header is always checked.
    pub fn cost_response_header(&self) -> Option<String> {
        None
    }

    /// Prose-ranker strategy (#895): extractive ranking when the embedding
    /// engine is available, else truncation.
    #[must_use]
    pub fn resolved_prose_ranker(&self) -> ProseRanker {
        ProseRanker::Auto
    }

    /// Output-savings holdout fraction (#895 Track B): none.
    #[must_use]
    pub fn output_holdout_fraction(&self) -> f64 {
        0.0
    }

    /// Whether the wire verbosity steer (#895) is enabled. No.
    #[must_use]
    pub fn verbosity_steer_enabled(&self) -> bool {
        false
    }

    /// Whether the cold-prefix repack (#480) is enabled. No: a wrong "cold"
    /// guess re-bills cache reads as writes (~12x).
    pub fn repacks_cold_prefix(&self) -> bool {
        false
    }

    /// Whether in-band CCR retrieval (#493) is enabled. No: the splice mutates
    /// provider-visible conversation content.
    pub fn ccr_inband_enabled(&self) -> bool {
        false
    }

    /// Whether Anthropic prompt-cache breakpoint injection (#939) is enabled.
    /// No: it mutates the provider-visible `system` shape.
    pub fn cache_breakpoint_enabled(&self) -> bool {
        false
    }

    /// Whether counterfactual savings metering (#701) is enabled. No.
    pub fn counterfactual_metering_enabled(&self) -> bool {
        false
    }

    /// Whether cache-aligner volatile-field telemetry (#940) is enabled. Yes:
    /// the scan is pure measurement and never mutates the body.
    pub fn cache_aligner_enabled(&self) -> bool {
        true
    }

    /// Whether active cache-aligner relocate (#974) is enabled. No: it
    /// reshapes the provider-visible `system` field.
    pub fn cache_align_relocate_enabled(&self) -> bool {
        false
    }

    /// Whether cache-economics (#986) is enabled: prompt-cache miss attribution
    /// telemetry plus the net-cost repack gate. Yes, both are strictly safe.
    pub fn cache_policy_enabled(&self) -> bool {
        true
    }

    /// Cross-provider reasoning effort (#834): off, a strict no-op that
    /// preserves the byte-unchanged meter-only path.
    #[must_use]
    pub fn resolved_effort(&self) -> Option<super::Effort> {
        None
    }

    /// Whether the proxy live-compresses non-protected `tool_result` content
    /// (#481). Yes.
    pub fn live_compresses(&self) -> bool {
        true
    }

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

    /// Never-compress path globs (#1150): none.
    #[must_use]
    pub fn compress_protect_globs(&self) -> Vec<glob::Pattern> {
        Vec::new()
    }

    /// Whether `path` is on the never-compress list (#1150). Never.
    #[must_use]
    pub fn is_path_compress_protected(&self, path: &str) -> bool {
        let _ = path;
        false
    }

    /// Prose-compression aggressiveness for `role`: off.
    #[must_use]
    pub fn resolved_role_aggressiveness(&self, role: ProseRole) -> Option<f64> {
        let _ = role;
        None
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

    /// Resolve the providers of [`PROVIDERS`]. An invalid URL is logged and the
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
    /// The providers of [`PROVIDERS`], validated and live-reloadable exactly
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
