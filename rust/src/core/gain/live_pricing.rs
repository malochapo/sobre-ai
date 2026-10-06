//! Live model prices from public provider catalogs (#1179, universal #1189).
//!
//! The embedded table in [`super::model_pricing`] can only ever know the
//! models that existed at release time; anything newer used to fall into a
//! family heuristic and could be priced an order of magnitude off (a tester's
//! DeepSeek V4 Flash was billed at 2025 V3 list prices — ~15× too high).
//!
//! Two public, key-less catalogs are fetched and merged (#1189):
//!
//! 1. `GET https://openrouter.ai/api/v1/models` — ~340 market-priced models
//!    covering every major vendor, incl. `:free`/`:extended` variants.
//!    Wins on key conflicts: it is market data refreshed continuously.
//! 2. The LiteLLM community price map (`model_prices_and_context_window.json`,
//!    ~2900 entries) — fills everything OpenRouter does not list: `azure/`,
//!    `bedrock/`, `vertex_ai/`, `groq/`, `mistral/`, embeddings, niche hosts.
//!
//! Either source failing is tolerated (partial refresh, fail-open); both
//! failing keeps the previous table. USD-per-token values are converted to
//! per-MTok [`ModelCost`] rows and cached on disk.
//!
//! Fetching happens **only** on `lean-ctx pricing refresh` — sobre makes no
//! network request the user did not ask for. The proxy loads whatever is
//! cached (`ensure_loaded`, no network); plain CLI tools and the test suite
//! keep the deterministic embedded table.
//!
//! Precedence inside [`super::model_pricing::ModelPricing::quote`]:
//! embedded exact > **live** > heuristic > blended fallback. Live hits are
//! [`super::model_pricing::PricingMatchKind::Live`] — current market data,
//! not an estimate.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use super::model_pricing::ModelCost;

/// On-disk cache file, under the lean-ctx cache directory.
const CACHE_FILE: &str = "model-prices.json";

const MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// LiteLLM community price map (#1189) — the de-facto industry catalog for
/// models OpenRouter does not route (Azure, Bedrock, Vertex, embeddings…).
const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// A fetched-and-indexed price table. `models` is keyed by canonicalized
/// lookup keys (see `canon`) — several keys may point at the same cost row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LivePriceTable {
    /// Unix seconds of the successful fetch that produced this table.
    pub fetched_at: u64,
    pub models: HashMap<String, ModelCost>,
}

impl LivePriceTable {
    /// Number of distinct lookup keys (not models) in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

fn snapshot() -> &'static RwLock<Option<Arc<LivePriceTable>>> {
    static SNAP: OnceLock<RwLock<Option<Arc<LivePriceTable>>>> = OnceLock::new();
    SNAP.get_or_init(|| RwLock::new(None))
}

/// Looks a model up in the current live snapshot. `None` when the snapshot
/// was never loaded (CLI tools, tests) or the model is genuinely unknown to
/// the provider list.
#[must_use]
pub fn lookup(model: &str) -> Option<(String, ModelCost)> {
    let guard = snapshot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let table = guard.as_ref()?;
    for key in lookup_candidates(model) {
        if let Some(cost) = table.models.get(&key) {
            return Some((key, *cost));
        }
    }
    None
}

/// Loads the disk cache into the process snapshot (idempotent, no network).
/// Returns the number of lookup keys now available. Call sites are the
/// run-modes that *want* live prices: proxy, gateway server, spend CLI.
pub fn ensure_loaded() -> usize {
    {
        let guard = snapshot()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(t) = guard.as_ref() {
            return t.len();
        }
    }
    let Some(table) = load_cache_file() else {
        return 0;
    };
    let len = table.len();
    install(table);
    len
}

/// Installs a table as the process snapshot (also used by tests).
pub fn install(table: LivePriceTable) {
    let mut guard = snapshot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some(Arc::new(table));
}

/// `(fetched_at_unix, lookup_keys)` of the active snapshot, for status
/// surfaces. `None` when no table was loaded.
#[must_use]
pub fn status() -> Option<(u64, usize)> {
    let guard = snapshot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().map(|t| (t.fetched_at, t.len()))
}

/// Test-only: clears the process snapshot so other tests see the embedded table.
#[cfg(test)]
pub fn clear_for_tests() {
    let mut guard = snapshot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = None;
}

fn cache_path() -> Option<std::path::PathBuf> {
    crate::core::paths::cache_dir()
        .ok()
        .map(|d| d.join(CACHE_FILE))
}

fn load_cache_file() -> Option<LivePriceTable> {
    let path = cache_path()?;
    let raw = std::fs::read(path).ok()?;
    let table: LivePriceTable = serde_json::from_slice(&raw).ok()?;
    if table.is_empty() { None } else { Some(table) }
}

/// Atomic write (tmp + rename) so a crashed refresh never truncates the cache.
fn store_cache_file(table: &LivePriceTable) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_err()
    {
        return;
    }
    let Ok(json) = serde_json::to_vec(table) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Fetches one catalog URL and parses it into a lookup map.
async fn fetch_catalog(
    client: &reqwest::Client,
    url: &str,
    parse: fn(&serde_json::Value) -> HashMap<String, ModelCost>,
) -> anyhow::Result<HashMap<String, ModelCost>> {
    let body = client
        .get(url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    let map = parse(&json);
    anyhow::ensure!(!map.is_empty(), "catalog {url} parsed empty");
    Ok(map)
}

/// Fetches both public catalogs, merges them (OpenRouter wins on conflicts —
/// market data including variants; LiteLLM fills the gaps: Azure, Bedrock,
/// Vertex, embeddings, niche hosts, #1189) and swaps the snapshot + disk
/// cache. One source failing is tolerated; the refresh only errors when *no*
/// source delivered anything.
///
/// # Errors
/// Network / decode errors propagate; the caller decides whether they matter
/// (`lean-ctx pricing refresh` reports them; the previous table is kept).
pub async fn refresh_now(client: &reqwest::Client) -> anyhow::Result<usize> {
    let (openrouter, litellm) = tokio::join!(
        fetch_catalog(client, MODELS_URL, parse_openrouter_models),
        fetch_catalog(client, LITELLM_URL, parse_litellm_models),
    );

    let mut models = match &openrouter {
        Ok(map) => map.clone(),
        Err(e) => {
            tracing::warn!("OpenRouter price catalog unavailable: {e:#}");
            HashMap::new()
        }
    };
    match &litellm {
        // `or_insert`: OpenRouter keys keep priority, LiteLLM extends coverage.
        Ok(map) => {
            for (k, v) in map {
                models.entry(k.clone()).or_insert(*v);
            }
        }
        Err(e) => tracing::warn!("LiteLLM price catalog unavailable: {e:#}"),
    }
    anyhow::ensure!(
        !models.is_empty(),
        "no price catalog reachable (OpenRouter: {}, LiteLLM: {})",
        openrouter
            .as_ref()
            .map_or_else(ToString::to_string, |m| format!("{} keys", m.len())),
        litellm
            .as_ref()
            .map_or_else(ToString::to_string, |m| format!("{} keys", m.len())),
    );

    let table = LivePriceTable {
        fetched_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        models,
    };
    let len = table.len();
    store_cache_file(&table);
    install(table);
    Ok(len)
}

/// Canonical lookup form: lowercase, `.`→`-`, whitespace→`-`. Applied to both
/// index keys and queries so `claude-opus-4.5` and `claude-opus-4-5` unify.
fn canon(s: &str) -> String {
    s.trim().to_lowercase().replace([' ', '.'], "-")
}

/// Strips a trailing `-YYYYMMDD` date stamp (`deepseek-v4-flash-20260423`).
fn strip_date_suffix(s: &str) -> Option<&str> {
    let (base, tail) = s.rsplit_once('-')?;
    if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) {
        Some(base)
    } else {
        None
    }
}

/// Ordered candidate keys for a query: exact canon first, then progressively
/// vendor-stripped / variant-stripped / date-stripped forms.
fn lookup_candidates(model: &str) -> Vec<String> {
    let full = canon(model);
    if full.is_empty() {
        return Vec::new();
    }
    let mut out = vec![full.clone()];
    let mut push = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    let no_vendor = full.split_once('/').map(|(_, m)| m.to_string());
    if let Some(nv) = &no_vendor {
        push(nv.clone());
    }
    for base in [Some(full.as_str()), no_vendor.as_deref()]
        .into_iter()
        .flatten()
    {
        let no_variant = base.split(':').next().unwrap_or(base);
        push(no_variant.to_string());
        if let Some(no_date) = strip_date_suffix(no_variant) {
            push(no_date.to_string());
        }
    }
    out
}

/// Index keys for one provider model id/slug. Mirrors [`lookup_candidates`]
/// so every form a client might send resolves. `with_variant` ids (`:free`)
/// only claim the variant-less keys when nothing else owns them.
fn index_keys(id: &str) -> (Vec<String>, bool) {
    let full = canon(id);
    let has_variant = full.contains(':');
    let mut keys = vec![full.clone()];
    let mut push = |s: String| {
        if !s.is_empty() && !keys.contains(&s) {
            keys.push(s);
        }
    };
    let no_vendor = full.split_once('/').map(|(_, m)| m.to_string());
    if let Some(nv) = &no_vendor {
        push(nv.clone());
    }
    for base in [Some(full.as_str()), no_vendor.as_deref()]
        .into_iter()
        .flatten()
    {
        let no_variant = base.split(':').next().unwrap_or(base);
        push(no_variant.to_string());
        if let Some(no_date) = strip_date_suffix(no_variant) {
            push(no_date.to_string());
        }
    }
    (keys, has_variant)
}

/// USD-per-token decimal string → USD per MTok. `None` for absent/invalid.
fn per_mtok(pricing: &serde_json::Value, field: &str) -> Option<f64> {
    let v = pricing.get(field)?;
    let n = v
        .as_str()
        .map_or_else(|| v.as_f64(), |s| s.trim().parse::<f64>().ok())?;
    if n.is_finite() && n >= 0.0 {
        Some(n * 1_000_000.0)
    } else {
        None
    }
}

/// Parses the OpenRouter `GET /api/v1/models` payload into the lookup map.
///
/// Pricing fields are USD-per-token strings; absent cache fields mean "no
/// separate cache pricing" — those tokens bill at the input rate (the same
/// convention the embedded table uses for Gemini/Foundry). `web_search` and
/// other per-request fees are not token prices and are ignored.
fn parse_openrouter_models(json: &serde_json::Value) -> HashMap<String, ModelCost> {
    let mut map: HashMap<String, ModelCost> = HashMap::new();
    let Some(data) = json.get("data").and_then(serde_json::Value::as_array) else {
        return map;
    };

    // Two passes: variant-less ids first so `model:free` never hijacks the
    // canonical `model` key; variants still resolve under their full name.
    let mut deferred: Vec<(&serde_json::Value, &str)> = Vec::new();
    let absorb = |map: &mut HashMap<String, ModelCost>, m: &serde_json::Value, id: &str| {
        let Some(pricing) = m.get("pricing") else {
            return;
        };
        let (Some(input), Some(output)) =
            (per_mtok(pricing, "prompt"), per_mtok(pricing, "completion"))
        else {
            return;
        };
        let cost = ModelCost {
            input_per_m: input,
            output_per_m: output,
            cache_write_per_m: per_mtok(pricing, "input_cache_write").unwrap_or(input),
            cache_read_per_m: per_mtok(pricing, "input_cache_read").unwrap_or(input),
        };
        let (keys, _) = index_keys(id);
        for key in keys {
            map.entry(key).or_insert(cost);
        }
        // The dated canonical slug resolves date-stamped client model names
        // (`deepseek-v4-flash-20260423`) even when the id carries no date.
        if let Some(slug) = m.get("canonical_slug").and_then(serde_json::Value::as_str) {
            let (slug_keys, _) = index_keys(slug);
            for key in slug_keys {
                map.entry(key).or_insert(cost);
            }
        }
    };

    for m in data {
        let Some(id) = m.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if canon(id).contains(':') {
            deferred.push((m, id));
        } else {
            absorb(&mut map, m, id);
        }
    }
    for (m, id) in deferred {
        absorb(&mut map, m, id);
    }
    map
}

/// A LiteLLM USD-per-token number field → USD per MTok. Unlike OpenRouter's
/// string prices these are plain JSON numbers; `null`/absent → `None`.
fn litellm_per_mtok(entry: &serde_json::Value, field: &str) -> Option<f64> {
    let n = entry.get(field)?.as_f64()?;
    if n.is_finite() && n >= 0.0 {
        Some(n * 1_000_000.0)
    } else {
        None
    }
}

/// Parses the LiteLLM `model_prices_and_context_window.json` map (#1189).
///
/// Top level is `{ "<model-key>": { input_cost_per_token, output_cost_per_token,
/// cache_read_input_token_cost, cache_creation_input_token_cost, mode, … } }`.
/// Keys carry LiteLLM's routing prefixes (`azure/gpt-4o`,
/// `bedrock/anthropic.claude-…`) which [`index_keys`] also resolves bare.
/// `sample_spec` is documentation, not a model. Entries priced per request /
/// per image / per second (no token prices) are skipped — the meter prices
/// tokens. Embedding rows (output cost 0) are kept: their input side is real.
fn parse_litellm_models(json: &serde_json::Value) -> HashMap<String, ModelCost> {
    let mut map: HashMap<String, ModelCost> = HashMap::new();
    let Some(entries) = json.as_object() else {
        return map;
    };
    for (key, entry) in entries {
        if key == "sample_spec" || !entry.is_object() {
            continue;
        }
        let Some(input) = litellm_per_mtok(entry, "input_cost_per_token") else {
            continue;
        };
        // Embeddings legitimately have no output price — bill output at 0
        // only when the mode says so; chat rows without output cost are junk.
        let output = match litellm_per_mtok(entry, "output_cost_per_token") {
            Some(o) => o,
            None if entry.get("mode").and_then(serde_json::Value::as_str) == Some("embedding") => {
                0.0
            }
            None => continue,
        };
        let cost = ModelCost {
            input_per_m: input,
            output_per_m: output,
            cache_write_per_m: litellm_per_mtok(entry, "cache_creation_input_token_cost")
                .unwrap_or(input),
            cache_read_per_m: litellm_per_mtok(entry, "cache_read_input_token_cost")
                .unwrap_or(input),
        };
        let (keys, _) = index_keys(key);
        for k in keys {
            map.entry(k).or_insert(cost);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        serde_json::json!({
            "data": [
                {
                    "id": "deepseek/deepseek-v4-flash",
                    "canonical_slug": "deepseek/deepseek-v4-flash-20260423",
                    "pricing": {"prompt": "0.00000007", "completion": "0.00000028",
                                 "input_cache_read": "0.000000007"}
                },
                {
                    "id": "anthropic/claude-sonnet-5",
                    "canonical_slug": "anthropic/claude-sonnet-5-20260630",
                    "pricing": {"prompt": "0.000002", "completion": "0.00001",
                                 "web_search": "0.01",
                                 "input_cache_read": "0.0000002",
                                 "input_cache_write": "0.0000025"}
                },
                {
                    "id": "poolside/laguna-xs-2.1:free",
                    "canonical_slug": "poolside/laguna-xs-2.1-20260625",
                    "pricing": {"prompt": "0", "completion": "0"}
                },
                {
                    "id": "poolside/laguna-xs-2.1",
                    "canonical_slug": "poolside/laguna-xs-2.1-20260625",
                    "pricing": {"prompt": "0.00000006", "completion": "0.00000012"}
                },
                {"id": "broken/no-pricing"}
            ]
        })
    }

    #[test]
    fn parses_usd_per_token_strings_into_per_mtok() {
        let map = parse_openrouter_models(&fixture());
        let flash = map.get("deepseek/deepseek-v4-flash").expect("indexed");
        assert!((flash.input_per_m - 0.07).abs() < 1e-9);
        assert!((flash.output_per_m - 0.28).abs() < 1e-9);
        assert!((flash.cache_read_per_m - 0.007).abs() < 1e-9);
        // No explicit cache write price → bills at the input rate.
        assert!((flash.cache_write_per_m - 0.07).abs() < 1e-9);
        assert!(!map.contains_key("broken/no-pricing"));
    }

    #[test]
    fn date_stamped_and_vendor_prefixed_names_resolve() {
        let map = parse_openrouter_models(&fixture());
        // The exact name Nicolas' client sent (#1179):
        for name in [
            "deepseek/deepseek-v4-flash-20260423",
            "deepseek-v4-flash-20260423",
            "deepseek-v4-flash",
        ] {
            let mut found = false;
            for key in lookup_candidates(name) {
                if map.contains_key(&key) {
                    found = true;
                    break;
                }
            }
            assert!(found, "{name} must resolve against the live table");
        }
    }

    #[test]
    fn free_variant_never_hijacks_the_paid_model_key() {
        let map = parse_openrouter_models(&fixture());
        let paid = map
            .get("poolside/laguna-xs-2-1")
            .expect("paid model indexed");
        assert!(
            paid.input_per_m > 0.0,
            "canonical key must carry the paid price"
        );
        let free = map
            .get("poolside/laguna-xs-2-1:free")
            .expect("variant indexed");
        assert_eq!(
            free.input_per_m, 0.0,
            "the :free variant stays free under its full name"
        );
    }

    #[test]
    fn dot_dash_and_case_unify() {
        assert_eq!(canon("Claude-Opus-4.5"), "claude-opus-4-5");
        assert_eq!(
            strip_date_suffix("deepseek-v4-flash-20260423"),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            strip_date_suffix("claude-opus-4-5"),
            None,
            "short numeric tails are versions"
        );
        assert_eq!(strip_date_suffix("no-date"), None);
    }

    fn litellm_fixture() -> serde_json::Value {
        serde_json::json!({
            "sample_spec": {
                "input_cost_per_token": 0.0,
                "output_cost_per_token": 0.0,
                "mode": "one of: chat, embedding, completion, …"
            },
            "azure/gpt-4o": {
                "input_cost_per_token": 2.5e-6,
                "output_cost_per_token": 1e-5,
                "cache_read_input_token_cost": 1.25e-6,
                "mode": "chat",
                "litellm_provider": "azure"
            },
            "bedrock/anthropic.claude-sonnet-4-5": {
                "input_cost_per_token": 3e-6,
                "output_cost_per_token": 1.5e-5,
                "cache_creation_input_token_cost": 3.75e-6,
                "cache_read_input_token_cost": 3e-7,
                "mode": "chat"
            },
            "text-embedding-3-small": {
                "input_cost_per_token": 2e-8,
                "mode": "embedding"
            },
            "vertex_ai/imagegeneration": {
                "output_cost_per_image": 0.02,
                "mode": "image_generation"
            }
        })
    }

    #[test]
    fn litellm_map_parses_prefixes_embeddings_and_skips_junk() {
        let map = parse_litellm_models(&litellm_fixture());

        // Azure deployment resolves under the prefixed AND the bare name.
        let azure = map.get("azure/gpt-4o").expect("prefixed key");
        assert!((azure.input_per_m - 2.5).abs() < 1e-9);
        assert!((azure.output_per_m - 10.0).abs() < 1e-9);
        assert!((azure.cache_read_per_m - 1.25).abs() < 1e-9);
        assert!(map.contains_key("gpt-4o"), "bare key indexed too");

        // Bedrock naming resolves; explicit cache-write price kept.
        let bedrock = map
            .get("bedrock/anthropic-claude-sonnet-4-5")
            .expect("bedrock key (canon: dots→dashes)");
        assert!((bedrock.cache_write_per_m - 3.75).abs() < 1e-9);

        // Embedding rows are real prices with zero output cost.
        let emb = map.get("text-embedding-3-small").expect("embedding");
        assert!((emb.input_per_m - 0.02).abs() < 1e-9);
        assert!((emb.output_per_m - 0.0).abs() < f64::EPSILON);

        // Documentation stub and per-image rows never enter the table.
        assert!(!map.contains_key("sample_spec"));
        assert!(!map.contains_key("vertex_ai/imagegeneration"));
    }

    #[test]
    fn merged_table_lets_openrouter_win_and_litellm_fill_gaps() {
        // Mirrors the merge in `refresh_now`: OpenRouter first, LiteLLM only
        // fills keys OpenRouter does not own.
        let mut merged = parse_openrouter_models(&fixture());
        for (k, v) in parse_litellm_models(&litellm_fixture()) {
            merged.entry(k).or_insert(v);
        }

        // Gap-fill: Azure exists only in LiteLLM.
        assert!(merged.contains_key("azure/gpt-4o"));
        // OpenRouter ownership survives: the fixture's deepseek price stays.
        let flash = merged
            .get("deepseek/deepseek-v4-flash")
            .expect("openrouter");
        assert!((flash.input_per_m - 0.07).abs() < 1e-9);
    }

    #[test]
    fn snapshot_lookup_respects_install() {
        let _lock = crate::core::data_dir::test_env_lock();
        clear_for_tests();
        assert!(
            lookup("deepseek/deepseek-v4-flash").is_none(),
            "empty snapshot"
        );

        install(LivePriceTable {
            fetched_at: 1,
            models: parse_openrouter_models(&fixture()),
        });
        let (_, cost) = lookup("deepseek/deepseek-v4-flash-20260423").expect("live hit");
        assert!((cost.input_per_m - 0.07).abs() < 1e-9);

        clear_for_tests();
    }
}
