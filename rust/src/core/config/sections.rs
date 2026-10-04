//! Auxiliary configuration section structs.
//!
//! Nested config structs (secret-detection, setup, archive, providers,
//! autonomy, updates, cloud, gain, loop-detection, embedding, …) split out of
//! `config/mod.rs` to keep the top-level module focused on `Config` itself.
//! Re-exported via `pub use sections::*`, so external paths stay stable.

use super::serde_defaults;
#[allow(clippy::wildcard_imports)]
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretDetectionConfig {
    pub enabled: bool,
    pub redact: bool,
    pub custom_patterns: Vec<String>,
    /// #718: subtractive counterpart to `custom_patterns` — a detected secret
    /// whose matched text is covered by any of these regexes is neither
    /// reported nor redacted. Lets users carve out known-safe identifiers or
    /// repo naming conventions without disabling secret detection wholesale.
    pub exclude_patterns: Vec<String>,
}

/// Controls what lean-ctx injects during `setup` and `update --rewire`.
/// Fresh installs default to non-invasive (rules/skills off, MCP on).
/// Users who ran setup interactively get explicit true/false.
/// `None` = undecided (legacy: check if rules already exist and preserve behavior).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SetupConfig {
    /// Inject agent rule files (CLAUDE.md, .cursor/rules/, etc.).
    /// None = undecided (legacy compat: inject if rules already present).
    /// Some(true) = always inject. Some(false) = never inject.
    pub auto_inject_rules: Option<bool>,
    /// Register lean-ctx as an MCP server in editor configs.
    #[serde(default = "serde_defaults::default_true")]
    pub auto_update_mcp: bool,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            auto_inject_rules: None,
            auto_update_mcp: true,
        }
    }
}

impl SetupConfig {
    /// Returns whether rules should be injected, considering legacy installs.
    /// If undecided (None), checks if lean-ctx rules markers already exist
    /// in any agent config — if so, keeps injecting for backward compat.
    pub fn should_inject_rules(&self) -> bool {
        match self.auto_inject_rules {
            Some(v) => v,
            None => Self::rules_already_present(),
        }
    }

    /// Returns whether `setup`/`onboard`/`init` may (re)register the lean-ctx
    /// MCP server in editor configs. Honors `auto_update_mcp` (#281) so locked-
    /// down environments can keep MCP out of agent settings while still getting
    /// hooks, rules and skills.
    pub fn should_update_mcp(&self) -> bool {
        self.auto_update_mcp
    }

    /// Check if lean-ctx rules markers exist in any known agent config location.
    ///
    /// Delegates the per-agent path catalog to `rules_inject::any_rules_marker_present`
    /// (derived from the injector's own target list) so this never drifts behind
    /// newly supported agents again (#442). Claude Code and CodeBuddy have no
    /// rules *target* (they auto-load an inline block instead), so their legacy
    /// rule files are checked separately to keep honoring older installs.
    fn rules_already_present() -> bool {
        let Some(home) = dirs::home_dir() else {
            return false;
        };
        if crate::rules_inject::any_rules_marker_present(&home) {
            return true;
        }
        let legacy_paths = [
            crate::core::editor_registry::claude_rules_dir(&home).join("lean-ctx.md"),
            crate::core::editor_registry::codebuddy_rules_dir(&home).join("lean-ctx.md"),
        ];
        legacy_paths.iter().any(|p| {
            std::fs::read_to_string(p)
                .is_ok_and(|c| c.contains(crate::core::rules_canonical::START_MARK))
        })
    }
}

impl Default for SecretDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            redact: true,
            custom_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
        }
    }
}

/// Settings for the zero-loss compression archive (large tool outputs saved to disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveConfig {
    pub enabled: bool,
    pub threshold_chars: usize,
    pub max_age_hours: u64,
    pub max_disk_mb: u64,
    pub ephemeral: bool,
    /// Minimum output tokens before the ephemeral firewall replaces an inline tool
    /// result with a summary + retrieval ref. Outputs below this stay fully inline.
    pub ephemeral_min_tokens: usize,
    /// Maximum output size that `ctx_shell(inline=true)` returns verbatim before
    /// the archive/firewall path takes over.
    pub inline_max_bytes: usize,
}

/// Opt-in conversation-history compression settings (#1123).
///
/// The proxy leaves conversation history byte-for-byte unchanged unless
/// `compression_enabled` is true and the configured token threshold is met.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationConfig {
    /// Enable message-level compression in the proxy. Default: false.
    pub compression_enabled: bool,
    /// Number of recent user turns (and their following messages) to preserve.
    pub preserve_last_n_turns: usize,
    /// Minimum estimated message-array size before compression starts.
    pub compression_threshold_tokens: usize,
    /// Minimum score for verbatim preservation.
    pub min_score_to_preserve: f64,
    /// Inclusive lower bound and exclusive upper bound for summaries.
    pub summarize_score_range: [f64; 2],
    /// Scores below this value are eligible for drop + CCR.
    pub drop_score_below: f64,
    /// Store dropped messages in the content-addressed recovery store.
    pub ccr_store_dropped: bool,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            compression_enabled: false,
            preserve_last_n_turns: 10,
            compression_threshold_tokens: 50_000,
            min_score_to_preserve: 0.5,
            summarize_score_range: [0.2, 0.5],
            drop_score_below: 0.2,
            ccr_store_dropped: true,
        }
    }
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_chars: 800,
            max_age_hours: 48,
            max_disk_mb: 500,
            ephemeral: true,
            ephemeral_min_tokens: 2000,
            inline_max_bytes: 32 * 1024,
        }
    }
}

impl ArchiveConfig {
    pub fn ephemeral_effective(&self) -> bool {
        if let Ok(v) = std::env::var("LEAN_CTX_EPHEMERAL") {
            return !matches!(v.trim(), "0" | "false" | "off");
        }
        self.ephemeral && self.enabled
    }

    pub fn ephemeral_min_tokens_effective(&self) -> usize {
        if let Ok(v) = std::env::var("LEAN_CTX_EPHEMERAL_MIN_TOKENS")
            && let Ok(n) = v.trim().parse::<usize>()
        {
            return n;
        }
        self.ephemeral_min_tokens
    }

    pub fn inline_max_bytes_effective(&self) -> usize {
        if let Ok(v) = std::env::var("LEAN_CTX_INLINE_MAX_BYTES")
            && let Ok(n) = v.trim().parse::<usize>()
        {
            return n;
        }
        self.inline_max_bytes
    }
}

/// Controls autonomous background behaviors (preload, dedup, consolidation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutonomyConfig {
    pub enabled: bool,
    pub auto_preload: bool,
    pub auto_dedup: bool,
    pub auto_related: bool,
    pub auto_consolidate: bool,
    pub silent_preload: bool,
    pub dedup_threshold: usize,
    pub consolidate_every_calls: u32,
    pub consolidate_cooldown_secs: u64,
    #[serde(default = "serde_defaults::default_true")]
    pub cognition_loop_enabled: bool,
    #[serde(default = "serde_defaults::default_cognition_loop_interval")]
    pub cognition_loop_interval_secs: u64,
    #[serde(default = "serde_defaults::default_cognition_loop_max_steps")]
    pub cognition_loop_max_steps: u8,
    /// Minimum facts an entity needs before observation synthesis (#802) writes a
    /// summary. Synthesis itself is gated by `cognition_loop_max_steps >= 9`.
    #[serde(default = "serde_defaults::default_cognition_synthesis_min_cluster")]
    pub cognition_synthesis_min_cluster: usize,
}

impl Default for AutonomyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_preload: true,
            auto_dedup: true,
            auto_related: true,
            auto_consolidate: true,
            silent_preload: true,
            dedup_threshold: 8,
            consolidate_every_calls: 25,
            consolidate_cooldown_secs: 120,
            cognition_loop_enabled: true,
            cognition_loop_interval_secs: 3600,
            cognition_loop_max_steps: 9,
            cognition_synthesis_min_cluster: 3,
        }
    }
}

/// Fixed-context budget accounting (#964). The per-session footprint lean-ctx
/// adds — tool schemas + MCP instructions + auto-loaded rules files + the wakeup
/// briefing — is warned about once it crosses `budget_tokens`. The
/// `LEAN_CTX_CONTEXT_BUDGET_TOKENS` env var overrides it; `lean-ctx doctor
/// overhead --gate` turns a breach into a non-zero exit for CI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub budget_tokens: usize,
    pub diet_max_config_tokens: usize,
    pub diet_relevance_threshold: f64,
    pub diet_rebalance_on_change: bool,
    pub diet_staleness_enabled: bool,
    /// Inject matching CCR archives into later tool responses.
    pub proactive_expansion: bool,
    /// Maximum proactive archive content per tool response.
    pub proactive_expansion_budget_tokens: usize,
    /// Minimum normalized BM25 score required for an injection.
    pub proactive_expansion_threshold: f64,
    /// Ignore archived content older than this many seconds; 0 disables age expiry.
    pub proactive_expansion_max_age_secs: u64,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            budget_tokens: 8000,
            diet_max_config_tokens: 800,
            diet_relevance_threshold: 0.15,
            diet_rebalance_on_change: true,
            diet_staleness_enabled: true,
            proactive_expansion: true,
            proactive_expansion_budget_tokens: 2000,
            proactive_expansion_threshold: 0.6,
            proactive_expansion_max_age_secs: 3600,
        }
    }
}

impl AutonomyConfig {
    /// Creates an autonomy config from env vars, falling back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("LEAN_CTX_AUTONOMY")
            && (v == "false" || v == "0")
        {
            cfg.enabled = false;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_AUTO_PRELOAD") {
            cfg.auto_preload = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_AUTO_DEDUP") {
            cfg.auto_dedup = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_AUTO_RELATED") {
            cfg.auto_related = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_AUTO_CONSOLIDATE") {
            cfg.auto_consolidate = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_SILENT_PRELOAD") {
            cfg.silent_preload = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_DEDUP_THRESHOLD")
            && let Ok(n) = v.parse()
        {
            cfg.dedup_threshold = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_CONSOLIDATE_EVERY_CALLS")
            && let Ok(n) = v.parse()
        {
            cfg.consolidate_every_calls = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_CONSOLIDATE_COOLDOWN_SECS")
            && let Ok(n) = v.parse()
        {
            cfg.consolidate_cooldown_secs = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_LOOP_ENABLED") {
            cfg.cognition_loop_enabled = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_LOOP_INTERVAL_SECS")
            && let Ok(n) = v.parse()
        {
            cfg.cognition_loop_interval_secs = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_LOOP_MAX_STEPS")
            && let Ok(n) = v.parse()
        {
            cfg.cognition_loop_max_steps = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_SYNTHESIS_MIN_CLUSTER")
            && let Ok(n) = v.parse()
        {
            cfg.cognition_synthesis_min_cluster = n;
        }
        cfg
    }

    /// Loads autonomy config from disk, with env var overrides applied.
    pub fn load() -> Self {
        let file_cfg = Config::load().autonomy;
        let mut cfg = file_cfg;
        if let Ok(v) = std::env::var("LEAN_CTX_AUTONOMY")
            && (v == "false" || v == "0")
        {
            cfg.enabled = false;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_AUTO_PRELOAD") {
            cfg.auto_preload = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_AUTO_DEDUP") {
            cfg.auto_dedup = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_AUTO_RELATED") {
            cfg.auto_related = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_SILENT_PRELOAD") {
            cfg.silent_preload = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_DEDUP_THRESHOLD")
            && let Ok(n) = v.parse()
        {
            cfg.dedup_threshold = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_LOOP_ENABLED") {
            cfg.cognition_loop_enabled = v != "false" && v != "0";
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_LOOP_INTERVAL_SECS")
            && let Ok(n) = v.parse()
        {
            cfg.cognition_loop_interval_secs = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_LOOP_MAX_STEPS")
            && let Ok(n) = v.parse()
        {
            cfg.cognition_loop_max_steps = n;
        }
        if let Ok(v) = std::env::var("LEAN_CTX_COGNITION_SYNTHESIS_MIN_CLUSTER")
            && let Ok(n) = v.parse()
        {
            cfg.cognition_synthesis_min_cluster = n;
        }
        cfg
    }
}

/// Settings for publishing your token-savings recap (`gain --publish` / auto-publish).
///
/// Publishing is always opt-in: it sends a small, whitelisted *aggregate* payload (tokens
/// saved, $ avoided, compression % — never code, paths or counts) to the cloud.
/// `auto_publish` simply removes the need to re-run `gain --publish` by hand; it stays off
/// until the user explicitly enables it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GainConfig {
    /// When true, `lean-ctx gain` automatically (re)publishes the recap, throttled to
    /// `auto_publish_interval_hours`. Off by default.
    pub auto_publish: bool,
    /// When auto-publishing, also opt into the public leaderboard.
    pub leaderboard: bool,
    /// Optional display name for the published card / leaderboard entry.
    pub display_name: Option<String>,
    /// Minimum hours between automatic publishes (throttle).
    pub auto_publish_interval_hours: u64,
    /// Runtime state — RFC3339 timestamp of the last automatic publish. Managed by the
    /// tool, not meant to be set by hand.
    pub last_auto_publish: Option<String>,
}

impl Default for GainConfig {
    fn default() -> Self {
        Self {
            auto_publish: false,
            leaderboard: true,
            display_name: None,
            auto_publish_interval_hours: 24,
            last_auto_publish: None,
        }
    }
}

/// Model declaration for **measured-vs-estimated** cost reporting.
///
/// Proxy-routed clients (Claude Code, Codex, Pi, Gemini CLI, OpenCode) report
/// their real model and billed tokens, so lean-ctx prices them *measured* with
/// no configuration. MCP-only IDEs (Cursor, Copilot, Windsurf, VS Code, Zed)
/// send their LLM traffic straight to the provider, bypassing lean-ctx — their
/// real model is invisible. Declaring it here lets those *estimated* turns be
/// priced with the correct model instead of a blended fallback.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CostConfig {
    /// Fallback pricing model for any client without a per-client entry.
    /// Unset/empty → lean-ctx keeps its blended heuristic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// Per-client pricing model, keyed by client id (`cursor`, `copilot`,
    /// `windsurf`, `claude`, `codex`, …). Used for MCP-only IDEs whose real
    /// model lean-ctx cannot observe. Example:
    /// `[cost.models]` then `cursor = "claude-opus-4.5"`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub models: HashMap<String, String>,
    /// Operator price overrides (#1189), keyed by model name — for negotiated
    /// enterprise rates (committed-use discounts, Azure PTU, zero-rated
    /// internal models) that no public catalog can know. Merged into the
    /// pricing table as **exact** entries, overriding embedded and live rows;
    /// only a provider-measured bill beats them. Example:
    /// `[cost.prices."internal-llm"]` then `input_per_m = 0.10`,
    /// `output_per_m = 0.40`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub prices: HashMap<String, PriceOverride>,
}

/// One `[cost.prices.<model>]` row: USD per million tokens. Omitted cache
/// rates default to the input rate (the same convention the catalogs use).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceOverride {
    pub input_per_m: Option<f64>,
    pub output_per_m: Option<f64>,
    pub cache_write_per_m: Option<f64>,
    pub cache_read_per_m: Option<f64>,
}

impl CostConfig {
    /// Configured pricing model for a client id: the per-client entry first, then
    /// the global default. `None` when neither is set (the caller then falls back
    /// to the env override / heuristic). Blank entries are ignored.
    pub fn model_for_client(&self, client: &str) -> Option<String> {
        self.models
            .get(client)
            .or(self.default_model.as_ref())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

/// Code-health engine (`[code_health]`): clean code as a token-cost lever.
///
/// Cognitive complexity and naming quality are computed once during
/// indexing and surfaced at read- and edit-time. These switches tune the
/// thresholds and how assertively findings are surfaced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeHealthConfig {
    /// Cognitive-complexity threshold above which a function is a hotspot.
    /// Mirrors `core::code_health::DEFAULT_COGNITIVE_THRESHOLD` (15).
    pub cognitive_threshold: u32,
    /// Edit-gate behavior on complexity drift: `"warn"` (annotate, default),
    /// `"block"` (refuse clean→over-threshold edits), or `"off"`.
    pub gate: String,
    /// Annotate over-threshold functions inline in `ctx_read` output.
    pub annotate_reads: bool,
    /// Run the naming-quality heuristic.
    pub naming: bool,
    /// Inject `[CODE HEALTH]` notices as `additionalContext` in PostToolUse stdout.
    /// Default: **false** — prevents prompt-cache invalidation on Anthropic models
    /// (#778: each injection causes 440-520k tokens of cache re-bills when Claude
    /// Code strips stale system-reminders retroactively).
    /// When false, notices route to `ctx_knowledge` + dashboard instead.
    #[serde(default)]
    pub inject_context: bool,
}

impl Default for CodeHealthConfig {
    fn default() -> Self {
        Self {
            cognitive_threshold: 15,
            gate: "warn".to_string(),
            annotate_reads: true,
            naming: true,
            inject_context: false,
        }
    }
}

/// Index-time file filters (#735): declare the retrieval corpus explicitly
/// instead of abusing `.gitignore` for retrieval policy.
///
/// Applies to every index builder through one shared filter layer
/// (`core::index_filter`): BM25, graph, and the watch/incremental path; the
/// semantic index chunks the BM25 corpus and inherits the same universe.
/// Excluded files never produce chunks, graph nodes, or embeddings. Globs are
/// matched against the root-relative path (forward slashes); exclude wins
/// over include. The empty default preserves today's behavior byte-for-byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    /// Honor `.gitignore` / global gitignore / `.git/info/exclude` during
    /// index walks. `false` indexes ignored files too (rarely wanted; the
    /// vendor-directory guard still applies).
    pub respect_gitignore: bool,
    /// Files to drop from the index corpus, e.g. `["**/*.csv", "fixtures/**"]`.
    /// Evaluated after `include`; a file matching both is excluded.
    pub exclude: Vec<String>,
    /// When non-empty, ONLY matching files enter the index corpus, e.g.
    /// `["**/*.rs", "**/*.ts"]`. Empty = no restriction.
    pub include: Vec<String>,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            exclude: Vec::new(),
            include: Vec::new(),
        }
    }
}

/// Settings for the code graph — in particular the *traversal* (co-access) edges
/// learned from real agent sessions (#289).
///
/// The static AST/import graph captures how code is wired structurally; it cannot
/// see which files an agent actually opens *together* while solving a task.
/// Traversal edges add that behavioural signal: files surfaced together are
/// associated with a decaying weight (Hebbian co-access), folded into the graph
/// as `co_access` edges and mixed into recall. The store is bounded and decays,
/// so stale associations fade.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphConfig {
    /// Record co-access between files surfaced together in a session, surface them
    /// as decaying `co_access` edges in the graph, and boost recall by them.
    /// On by default; set to `false` for a purely static (AST-only) graph.
    pub traversal_edges: bool,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            traversal_edges: true,
        }
    }
}

/// AI session summaries (#292): periodically distil the working session into a
/// compact, *semantically recallable* summary so a future session can answer
/// "what did I do last time on X?". Deterministic and local-first — recall uses
/// embeddings when the `embeddings` feature is on, else a lexical fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SummariesConfig {
    /// Record periodic session summaries. On by default; recording is cheap and
    /// happens at most once per `every_n_turns` tool calls.
    pub enabled: bool,
    /// Tool calls between automatic summaries. The auto-checkpoint cadence still
    /// gates the check, so the effective minimum is the checkpoint interval.
    pub every_n_turns: u32,
    /// Maximum summaries kept per project (oldest pruned first).
    pub max_kept: u32,
}

impl Default for SummariesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            every_n_turns: 25,
            max_kept: 100,
        }
    }
}

/// Thresholds for detecting and throttling repetitive agent tool call loops.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopDetectionConfig {
    pub normal_threshold: u32,
    pub reduced_threshold: u32,
    pub blocked_threshold: u32,
    pub window_secs: u64,
    pub search_group_limit: u32,
    pub tool_total_limits: HashMap<String, u32>,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        let mut tool_total_limits = HashMap::new();
        tool_total_limits.insert("ctx_read".to_string(), 100);
        tool_total_limits.insert("ctx_search".to_string(), 80);
        tool_total_limits.insert("ctx_shell".to_string(), 50);
        tool_total_limits.insert("ctx_semantic_search".to_string(), 60);
        Self {
            normal_threshold: 2,
            reduced_threshold: 4,
            blocked_threshold: 0,
            window_secs: 300,
            search_group_limit: 10,
            tool_total_limits,
        }
    }
}

/// Semantic-embedding engine settings.
///
/// `model` selects which local ONNX embedding model lean-ctx downloads and uses for
/// `ctx_semantic_search`. Accepts the same aliases as the `LEAN_CTX_EMBEDDING_MODEL` env
/// var: `minilm` (all-MiniLM-L6-v2, 384d — the default), `nomic` (768d) — or any
/// HuggingFace repo with an ONNX export via `hf:org/repo[@revision]` (GL #397), e.g.
/// `hf:jinaai/jina-embeddings-v2-base-code` for code-specialized embeddings. When the
/// env var is set it takes precedence; an
/// unset/`None` value uses the default model. Switching models triggers a one-time
/// re-index on the next semantic search (vector dimensions follow from the model).
///
/// `dimensions` is only consulted for `hf:` custom models as the declared fallback
/// width; the real width is probed from the ONNX graph at load time. Built-ins ignore it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<usize>,
    /// Allow downloading the embedding model on first semantic need (#551).
    /// `None` (unset) means **allowed** — the soft default that activates the
    /// semantic features without manual setup. Set `false` for air-gapped
    /// machines. The `LEAN_CTX_EMBEDDINGS_AUTO_DOWNLOAD` env var, when set,
    /// overrides this in either direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_download: Option<bool>,
    /// Pin embedding inference to a single CPU thread (no GPU EP) so vectors are
    /// bit-identical across machines, not just run-to-run on one host (#895).
    /// `None`/`false` keeps the multi-threaded GPU-capable path. Extractive prose
    /// ranking is already deterministic via score quantization + stable tiebreak;
    /// this flag is the extra hardening for cross-machine reproducibility. The
    /// `LEAN_CTX_EMBEDDING_DETERMINISTIC` env var overrides this either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deterministic: Option<bool>,
}
