//! API proxy upstream overrides (`config.toml`).

use serde::{Deserialize, Serialize};

/// API proxy upstream overrides. `None` = use provider default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub anthropic_upstream: Option<String>,
    pub openai_upstream: Option<String>,
    pub gemini_upstream: Option<String>,
    /// Universal provider registry (`[[proxy.providers]]`): additional upstream
    /// providers beyond the four built-ins, declared as data — id + wire shape +
    /// base URL — so a new OpenAI/Anthropic/Gemini/Bedrock endpoint (Azure AI
    /// Foundry, OpenRouter, Groq, vLLM/Ollama, a corporate gateway…) is a pure
    /// config entry, never a code change. Reachable under
    /// `/providers/{id}/...` on the proxy and addressable by the router.
    /// The legacy `*_upstream` fields above stay authoritative for the four
    /// built-in provider routes (backwards compatible).
    pub providers: Vec<ProviderEntry>,
    /// Allow a non-loopback plaintext `http://` upstream (trusted local network
    /// only). Opt-in; see [`ProxyConfig::allows_insecure_http_upstream`]. (#440)
    pub allow_insecure_http_upstream: Option<bool>,
    /// Allow a custom (non-allowlisted) **HTTPS** upstream host — e.g. a corporate
    /// gateway in front of the provider API. Opt-in; see
    /// [`ProxyConfig::allows_custom_upstream`]. Mirrors `allow_insecure_http_upstream`
    /// so the long-lived managed proxy (LaunchAgent / systemd), which only reads
    /// `config.toml`, can honor a custom upstream too (#590).
    pub allow_custom_upstream: Option<bool>,
    /// Active request routing (`[proxy.routing]`, enterprise#13): model aliases
    /// and intent-tier downgrades applied in the forward path. Off by default —
    /// an empty/absent table is a strict passthrough. See [`RoutingRules`].
    pub routing: RoutingRules,
}

/// `[proxy.routing]` — the active router's rule set (enterprise#13).
///
/// Two mechanisms, both **within-shape** in M1 (the target must speak the same
/// wire dialect as the request; N×M shape translation is M2):
///
/// - **Aliases**: exact requested-model → target. Lets an org expose stable
///   names (`acme/fast`) or transparently swap one concrete model for another.
/// - **Tiers**: intent-based downgrade. The request's last user message is
///   classified (`intent_router`); the resulting tier (`fast|standard|premium`)
///   picks a target from this table. An absent tier key (or `""`) keeps the
///   requested model — premium work is never silently downgraded unless the
///   operator says so.
///
/// A target is `"model"` (swap the model, keep the upstream) or
/// `"provider:model"` where `provider` is a `[[proxy.providers]]` registry id
/// or a built-in (`anthropic|openai|gemini`) — then the request is also
/// re-targeted to that provider's upstream.
///
/// **Fail-open by construction:** any lookup/classification/validation miss
/// routes nothing and forwards the request unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoutingRules {
    /// Master switch; `false`/absent = passthrough (no body rewrite at all).
    pub enabled: Option<bool>,
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
        self.enabled.unwrap_or(false) && !(self.aliases.is_empty() && self.tiers.is_empty())
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

/// One `[[proxy.providers]]` registry entry (see [`ProxyConfig::providers`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    /// Registry id, used in the `/providers/{id}/...` route and in routing
    /// rules. Lowercase alphanumeric plus `-`/`_`; must not shadow a built-in
    /// provider name (`anthropic`, `openai`, `gemini`).
    pub id: String,
    /// Which API dialect the endpoint speaks (`anthropic|openai|gemini|bedrock`).
    pub shape: WireShape,
    /// Endpoint base URL. HTTPS for any non-loopback host; a declared registry
    /// entry is itself the custom-host opt-in (no separate allowlist flag).
    pub base_url: String,
    /// Name of the environment variable holding the upstream API key the
    /// gateway injects (replacing the caller's credential headers). `None` =
    /// forward the caller's own credentials verbatim (default, loopback mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// AWS region for Bedrock SigV4. Bedrock uses standard AWS credential
    /// environment variables and rejects `api_key_env`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,
    /// Set `false` to keep the entry in config but take it out of service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Marks this endpoint as local inference (Ollama/vLLM/…): usage is booked
    /// at the transparent `local_shadow_rate` instead of provider list prices
    /// (enterprise#15/#18). Unset = derived from the URL (loopback hosts are
    /// local). Set it explicitly when the endpoint is local but not loopback —
    /// the containerized gateway reaching the host's Ollama
    /// (`host.docker.internal`) or an in-cluster server (`ollama.svc.cluster.local`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<bool>,
}

/// A validated, ready-to-serve registry provider (runtime view of
/// [`ProviderEntry`], published inside [`Upstreams`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub id: String,
    pub shape: WireShape,
    pub base_url: String,
    pub api_key_env: Option<String>,
    /// Validated SigV4 region for Bedrock; absent for other shapes.
    pub aws_region: Option<String>,
    /// Billed as local inference (shadow rate). Explicit `local` flag when the
    /// entry declares one, otherwise loopback-URL derivation.
    pub local: bool,
}

/// Built-in provider route names a registry entry must not shadow.
const BUILTIN_PROVIDER_IDS: &[&str] = &["anthropic", "openai", "gemini"];

/// True when `id` is usable as a registry id: non-empty, lowercase alnum plus
/// `-`/`_` (it becomes a URL path segment), and not a built-in provider name.
fn is_valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && !BUILTIN_PROVIDER_IDS.contains(&id)
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn valid_aws_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_bedrock_endpoint(base_url: &str, region: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    let loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if loopback {
        return true;
    }
    let accepted = [
        format!("bedrock-runtime.{region}.amazonaws.com"),
        format!("bedrock-runtime.{region}.amazonaws.com.cn"),
        format!("bedrock-runtime.{region}.api.aws"),
        format!("bedrock-runtime-fips.{region}.amazonaws.com"),
        format!("bedrock-runtime-fips.{region}.api.aws"),
    ];
    accepted.iter().any(|candidate| candidate == &host)
        || host.ends_with(&format!(".bedrock-runtime.{region}.vpce.amazonaws.com"))
}

impl ResolvedProvider {
    #[must_use]
    pub fn injects_gateway_credential(&self) -> bool {
        self.api_key_env.is_some() || self.shape == WireShape::Bedrock
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

    /// Whether a non-loopback plaintext `http://` upstream is allowed. Opt-in
    /// only — a deliberate downgrade for a trusted local-network service such as
    /// `http://host.docker.internal:2455` in front of codex-lb (#440).
    /// `[proxy] allow_insecure_http_upstream` in config.toml, default `false`.
    pub fn allows_insecure_http_upstream(&self) -> bool {
        self.allow_insecure_http_upstream.unwrap_or(false)
    }

    /// Whether a custom (non-allowlisted) HTTPS upstream host is allowed. Opt-in
    /// only — lifting the built-in host allowlist points the proxy at a host you
    /// control (e.g. a corporate gateway), so it must be deliberate.
    /// `[proxy] allow_custom_upstream` in config.toml, default `false`.
    pub fn allows_custom_upstream(&self) -> bool {
        self.allow_custom_upstream.unwrap_or(false)
    }

    /// True when any `*_upstream` configured in `config.toml` is a
    /// custom HTTPS host outside the built-in allowlist — i.e. one that resolves
    /// only with the [`Self::allows_custom_upstream`] opt-in. Plaintext-HTTP custom
    /// hosts are governed by `allow_insecure_http_upstream` instead, so they are
    /// excluded here. Lets `proxy enable`/`restart` persist the opt-in (so the
    /// managed proxy honors it) and `proxy status` explain a blocked upstream,
    /// without touching the allowlisted-host case (#590).
    #[must_use]
    pub fn has_custom_host_upstream(&self) -> bool {
        [
            self.anthropic_upstream.as_deref(),
            self.openai_upstream.as_deref(),
            self.gemini_upstream.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(normalize_url_opt)
        .any(|u| is_custom_upstream_host(&u))
    }

    /// `(configured value, provider default)` for one provider.
    fn provider_spec(&self, provider: ProxyProvider) -> (Option<&str>, &'static str) {
        match provider {
            ProxyProvider::Anthropic => (
                self.anthropic_upstream.as_deref(),
                "https://api.anthropic.com",
            ),
            ProxyProvider::OpenAi => (self.openai_upstream.as_deref(), "https://api.openai.com"),
            ProxyProvider::Gemini => (
                self.gemini_upstream.as_deref(),
                "https://generativelanguage.googleapis.com",
            ),
        }
    }

    /// Resolve one upstream: `[proxy].*_upstream` (config.toml) > provider
    /// default.
    /// Returns `Err` when a value is *present but invalid* so a live reload can
    /// keep the last good value instead of silently rerouting to the default; an
    /// *absent* value resolves to the provider default (`Ok`).
    fn resolve_upstream_checked(&self, provider: ProxyProvider) -> Result<String, String> {
        let (config_val, default) = self.provider_spec(provider);
        match config_val.and_then(normalize_url_opt) {
            None => Ok(normalize_url(default)),
            Some(url) => validate_upstream_url(
                &url,
                self.allows_insecure_http_upstream(),
                self.allows_custom_upstream(),
            ),
        }
    }

    /// Effective upstream for a provider (config > default). An invalid
    /// configured value falls back to the provider default (logged) — the
    /// safe choice at startup.
    pub fn resolve_upstream(&self, provider: ProxyProvider) -> String {
        match self.resolve_upstream_checked(provider) {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!("upstream validation failed, using default: {e}");
                normalize_url(self.provider_spec(provider).1)
            }
        }
    }

    /// Resolve all three upstreams at once.
    pub fn resolve_all(&self) -> Upstreams {
        Upstreams {
            anthropic: self.resolve_upstream(ProxyProvider::Anthropic),
            openai: self.resolve_upstream(ProxyProvider::OpenAi),
            gemini: self.resolve_upstream(ProxyProvider::Gemini),
            providers: self.resolve_providers(),
        }
    }

    /// Validate + resolve the `[[proxy.providers]]` registry. Invalid entries
    /// are logged and skipped (one typo must never take the proxy down or
    /// disable the remaining registry); duplicates keep the first occurrence.
    /// A declared registry entry is itself the deliberate custom-host opt-in,
    /// so any HTTPS host is accepted; plaintext HTTP still requires loopback or
    /// the explicit insecure-HTTP opt-in (same rule as the built-ins).
    #[must_use]
    pub fn resolve_providers(&self) -> Vec<ResolvedProvider> {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for entry in &self.providers {
            if !entry.enabled.unwrap_or(true) {
                continue;
            }
            let id = entry.id.trim();
            if !is_valid_provider_id(id) {
                tracing::warn!(
                    "[proxy.providers] invalid id '{id}' (lowercase alnum/-/_ only, \
                     must not shadow a built-in provider) — entry skipped"
                );
                continue;
            }
            if !seen.insert(id) {
                tracing::warn!("[proxy.providers] duplicate id '{id}' — keeping first entry");
                continue;
            }
            match validate_upstream_url(&entry.base_url, self.allows_insecure_http_upstream(), true)
            {
                Ok(base_url) => {
                    let aws_region = entry
                        .aws_region
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    if entry.shape == WireShape::Bedrock {
                        let Some(region) = aws_region.filter(|value| valid_aws_region(value))
                        else {
                            tracing::warn!(
                                "[proxy.providers] Bedrock provider '{id}' requires valid aws_region — skipped"
                            );
                            continue;
                        };
                        if entry.api_key_env.is_some() || !valid_bedrock_endpoint(&base_url, region)
                        {
                            tracing::warn!(
                                "[proxy.providers] Bedrock provider '{id}' has invalid credential mode or endpoint — skipped"
                            );
                            continue;
                        }
                    } else if aws_region.is_some() {
                        tracing::warn!(
                            "[proxy.providers] non-Bedrock provider '{id}' cannot set aws_region — skipped"
                        );
                        continue;
                    }
                    // Explicit `local` flag wins; otherwise loopback URLs are
                    // local (host.docker.internal etc. need the explicit flag).
                    let local = entry.local.unwrap_or_else(|| is_local_proxy_url(&base_url));
                    out.push(ResolvedProvider {
                        id: id.to_string(),
                        shape: entry.shape,
                        base_url,
                        api_key_env: entry
                            .api_key_env
                            .as_deref()
                            .map(str::trim)
                            .filter(|v| !v.is_empty())
                            .map(str::to_string),
                        aws_region: aws_region.map(str::to_string),
                        local,
                    });
                }
                Err(e) => {
                    tracing::warn!("[proxy.providers] '{id}' has invalid base_url — skipped: {e}");
                }
            }
        }
        out
    }

    /// Re-resolve upstreams for a *running* proxy (#449). For any provider whose
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
            // Registry re-resolution is deterministic from config; an entry
            // that turned invalid is dropped with a warning (see
            // `resolve_providers`), the rest keep serving.
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
    /// Registry providers from `[[proxy.providers]]` (universal framework),
    /// validated and live-reloadable exactly like the built-ins.
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

const ALLOWED_UPSTREAM_HOSTS: &[&str] = &[
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    // xAI Grok (OpenAI-compatible). Used by auto-seeded registry providers
    // when `lean-ctx proxy enable` wires Grok (API-key rail + subscription rail).
    "api.x.ai",
    "cli-chat-proxy.grok.com",
    // Command Code gateway. Used by the auto-seeded `commandcode` registry
    // provider when `lean-ctx proxy enable` wires Command Code.
    "api.commandcode.ai",
];

pub(super) fn validate_upstream_url(
    url: &str,
    allow_insecure_http: bool,
    allow_custom_host: bool,
) -> Result<String, String> {
    let normalized = normalize_url(url);
    // Loopback HTTP never leaves the machine — always allowed.
    if is_local_proxy_url(&normalized) {
        return Ok(normalized);
    }

    // A non-loopback plaintext `http://` upstream is reachable only through the
    // explicit opt-in (#440). The old code rejected it on the HTTPS check *before*
    // any override could apply. Handle it up front: the opt-in
    // implies a deliberate custom host on a trusted local network, so it needs no
    // separate allowlist check; otherwise give a hint that actually works.
    if normalized.starts_with("http://") {
        if allow_insecure_http {
            return Ok(normalized);
        }
        return Err(format!(
            "upstream URL must use HTTPS: {normalized} (for a trusted local-network HTTP \
             upstream opt in with `[proxy] allow_insecure_http_upstream = true`)"
        ));
    }
    let Some(host_segment) = normalized.strip_prefix("https://") else {
        return Err(format!(
            "upstream URL must start with http:// or https://: {normalized}"
        ));
    };

    let host = host_segment.split('/').next().unwrap_or("");
    let host_no_port = host.split(':').next().unwrap_or(host);
    if ALLOWED_UPSTREAM_HOSTS.contains(&host_no_port) || allow_custom_host {
        Ok(normalized)
    } else {
        Err(format!(
            "upstream host '{host_no_port}' not in allowlist {ALLOWED_UPSTREAM_HOSTS:?} (for a \
             custom upstream host opt in with `[proxy] allow_custom_upstream = true`)"
        ))
    }
}

/// True when `url` is an HTTPS upstream whose host is not in the built-in
/// allowlist (and not loopback) — the case the `allow_custom_upstream` opt-in
/// governs. Plaintext-HTTP custom hosts are governed by
/// `allow_insecure_http_upstream` instead, so they are excluded here.
fn is_custom_upstream_host(url: &str) -> bool {
    let n = normalize_url(url);
    if is_local_proxy_url(&n) {
        return false;
    }
    let Some(host_segment) = n.strip_prefix("https://") else {
        return false;
    };
    let host = host_segment.split('/').next().unwrap_or("");
    let host_no_port = host.split(':').next().unwrap_or(host);
    !host_no_port.is_empty() && !ALLOWED_UPSTREAM_HOSTS.contains(&host_no_port)
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
