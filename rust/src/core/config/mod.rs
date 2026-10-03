use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::memory_policy::MemoryPolicy;

/// Cache payload for [`Config::load_arc`]: the shared config alongside the
/// content hashes of the global and project-local files it was built from, so a
/// later load re-reads + re-hashes and only rebuilds on a real change (#406).
type ConfigCacheSlot = Option<(Arc<Config>, Option<String>, Option<String>)>;

mod enums;
mod loader;
mod memory;
mod merge;
mod provenance;
mod proxy;
mod read_dedup;
pub(crate) mod read_redirect;
mod render;
mod response_shaping;
pub mod risk;
pub mod schema;
mod sections;
mod serde_defaults;
pub mod setter;
mod shell_activation;
pub use render::render_annotated_config;
pub use sections::*;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_parsing;

pub use enums::{
    CompressionLevel, Effort, OutputDensity, PermissionInheritance, RecoveryHints,
    ResponseVerbosity, RulesInjection, RulesScope, SessionDegrade, TeeMode, TerseAgent,
};
pub use loader::last_config_parse_error;
pub use loader::local_sensitive_overrides;
pub(crate) use loader::strip_sensitive_overrides;
pub use memory::{MemoryCleanup, MemoryGuardConfig, MemoryProfile, SavingsFooter};
pub use provenance::{ConfigProvenance, EnvOverride};
pub use proxy::{
    BaselineConfig, DEFAULT_LOCAL_SHADOW_RATE_PER_MTOK, HistoryMode, ProseRanker, ProseRole,
    ProviderEntry, ProxyConfig, ProxyMode, ProxyProvider, ResolvedProvider, RoleAggressiveness,
    RoutingRules, UpstreamDrift, Upstreams, WireShape, diagnose_drift, env_upstream_override,
    is_local_proxy_url, normalize_url, normalize_url_opt, parse_route_target,
};
pub use read_dedup::ReadDedup;
pub use read_redirect::ReadRedirect;
pub use response_shaping::{
    CodeRepetitionConfig, ConfirmationConfig, NarrationConfig, PreambleConfig,
    ResponseShapingConfig,
};
pub use shell_activation::ShellActivation;

/// Default BM25 cache cap from config (also used by `bm25_index` heuristics).
pub fn default_bm25_max_cache_mb() -> u64 {
    serde_defaults::default_bm25_max_cache_mb()
}

/// Effective on-disk ceiling (MB) for the persisted BM25 index when nothing is
/// explicitly configured (no `bm25_max_cache_mb`, no `max_disk_mb` budget).
///
/// Deliberately decoupled from the RAM `MemoryProfile` (64/128/512 MB): this is
/// a *disk* file, and tying it to the profile silently refused persistence on
/// large repos under Low/Balanced, forcing a cold rebuild on every call (the
/// perpetual "index warming" of issue #249). 512 MB compressed covers
/// essentially every real repo; RAM pressure is governed separately by the
/// eviction orchestrator (which measures real heap).
pub const DEFAULT_BM25_PERSIST_MB: u64 = 512;

// Compile-time regression guard (#249): the default disk ceiling must stay well
// above the old RAM-profile caps (64/128 MB) that starved large repos.
const _: () = assert!(DEFAULT_BM25_PERSIST_MB >= 512);

/// lean-ctx tools whose sole purpose is editing the user's source files. When
/// `prefer_native_editor` is set (#454) these are hidden from `list_tools` and
/// refused at dispatch so the host's native editor handles edits instead.
///
/// Deliberately narrow: only the dedicated edit tools are blocked — `ctx_edit`
/// (str_replace) and `ctx_patch` (anchored, #1008). LSP refactor
/// (`ctx_refactor`) also exposes read-only sub-actions (references/definition),
/// so it is left available.
pub const EDIT_TOOL_NAMES: &[&str] = &["ctx_edit", "ctx_patch"];

/// Global lean-ctx configuration loaded from `config.toml`, merged with project-local overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(default, deserialize_with = "serde_defaults::deserialize_tee_mode")]
    pub tee_mode: TeeMode,
    /// Verbosity of the reactive recovery footer on compressed output
    /// (`off|minimal|full`, default `minimal`). See [`RecoveryHints`].
    #[serde(default)]
    pub recovery_hints: RecoveryHints,
    #[serde(default)]
    pub output_density: OutputDensity,
    pub checkpoint_interval: u32,
    pub passthrough_commands: Vec<String>,
    pub passthrough_urls: Vec<String>,
    /// Output formats that are already compact/token-oriented and must be
    /// preserved verbatim instead of being recompressed (#342). Matched against
    /// the *output shape* (not the command name), so any tool emitting the
    /// format is covered without enumerating commands in `passthrough_commands`.
    /// Default: `["toon"]`. Set to `[]` to disable and always recompress.
    #[serde(default = "serde_defaults::default_preserve_compact_formats")]
    pub preserve_compact_formats: Vec<String>,
    /// Opt-in: apply the lossless JSON crusher to *verbatim* data commands
    /// (`gh api`, `jq`, `kubectl get -o json`, `curl` JSON). Off by default, so
    /// those outputs stay byte-for-byte verbatim. When on, an array-heavy JSON
    /// payload the crusher can at least halve is reshaped into a compact, fully
    /// reconstructible form; everything else stays verbatim. See
    /// [`Config::crush_verbatim_json_effective`] (#936).
    #[serde(default)]
    pub crush_verbatim_json: bool,
    /// Commands taking longer than this threshold (ms) are recorded in the slow log.
    /// Set to 0 to disable slow logging.
    pub slow_command_threshold_ms: u64,
    #[serde(default = "serde_defaults::default_theme")]
    pub theme: String,
    #[serde(default)]
    pub gain: GainConfig,
    /// Model declaration for measured-vs-estimated cost reporting (MCP-only IDEs).
    #[serde(default)]
    pub cost: CostConfig,
    /// Code-health engine: cognitive complexity, naming, coupling, edit-gate.
    #[serde(default)]
    pub code_health: CodeHealthConfig,
    #[serde(default)]
    pub autonomy: AutonomyConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Conversation-history compression (`[conversation]`, opt-in; #1123).
    #[serde(default)]
    pub conversation: ConversationConfig,
    /// Proxy-layer response shaping (`[response_shaping]`, #1125).
    #[serde(default)]
    pub response_shaping: ResponseShapingConfig,
    /// Whether the API proxy is enabled. Tri-state:
    /// - None: undecided (fresh install, will prompt on interactive setup)
    /// - Some(true): user opted in, proxy managed by lean-ctx
    /// - Some(false): user opted out, never touch proxy or endpoints
    #[serde(default)]
    pub proxy_enabled: Option<bool>,
    #[serde(default)]
    pub proxy_port: Option<u16>,
    /// Proxy reachability timeout in milliseconds. Default: 200.
    /// Override via LEAN_CTX_PROXY_TIMEOUT_MS env var.
    #[serde(default)]
    pub proxy_timeout_ms: Option<u64>,
    /// Strict proxy auth: when true, authenticate ONLY via the Bearer token
    /// (`LEAN_CTX_PROXY_TOKEN`) and disable the provider-API-key fallback. Default
    /// false keeps the loopback-friendly behavior where any local AI tool's own
    /// provider key authenticates (the proxy never injects upstream credentials —
    /// it forwards the caller's key verbatim). Enable on shared/multi-user hosts to
    /// require the token; clients must then send `Authorization: Bearer <token>`.
    #[serde(default)]
    pub proxy_require_token: bool,
    /// Skip ALL proxy authentication on loopback-bound listeners (#755).
    /// When true **and** the proxy binds a loopback address, every request is
    /// accepted without a Bearer token or provider API key — MCP clients,
    /// browser dashboards, and CLI tools all work without auth setup.
    /// Env override: `LEAN_CTX_PROXY_LOOPBACK_OPEN`.
    #[serde(default)]
    pub proxy_loopback_open: bool,
    /// Provider prompt-cache hit rate for net-of-injection calculation (#1104).
    /// Anthropic ~90%, OpenAI ~50%. Default 0.75 (conservative cross-provider).
    #[serde(default)]
    pub dashboard_cache_hit_rate: Option<f64>,
    #[serde(default = "serde_defaults::default_true")]
    pub enable_wakeup_ctx: bool,
    /// Prefer the host agent's native editor over lean-ctx edit operations (#454).
    /// When true, the lean-ctx edit tool(s) (see [`EDIT_TOOL_NAMES`]) are neither
    /// advertised in `list_tools` nor dispatchable (direct or via `ctx_call`), so
    /// the agent falls back to the host's built-in editing UI. Reads / search /
    /// shell / memory tools are unaffected. Override via
    /// `LEAN_CTX_PREFER_NATIVE_EDITOR=1`.
    #[serde(default)]
    pub prefer_native_editor: bool,
    /// Tool categories to activate by default for dynamic-tool-capable clients.
    /// Values: "core" (always on), "arch", "debug", "memory", "metrics", "session".
    /// Example: `default_tool_categories = ["core", "arch", "memory"]`
    /// Override via LCTX_DEFAULT_CATEGORIES env var (comma-separated).
    /// Empty = lean-ctx default (core + session).
    #[serde(default)]
    pub default_tool_categories: Vec<String>,
    /// Disable all automatic read-mode degradation (auto_degrade + context_gate pressure).
    /// When true, lean-ctx never downgrades requested read modes regardless of pressure.
    /// Override via LCTX_NO_DEGRADE=1 env var.
    #[serde(default)]
    pub no_degrade: bool,
    /// Serve explicit `full`/`lines:N-M` re-reads of session-cached files as
    /// deltas: when the file changed on disk since it was cached, the read
    /// returns `mode=diff` instead of re-emitting content the model already
    /// holds. First reads are unaffected; `fresh=true` always bypasses.
    /// Opt-in. Override via LCTX_DELTA_EXPLICIT=1/0 env var.
    #[serde(default)]
    pub delta_explicit: bool,
    /// Persistent profile name. Checked after LEAN_CTX_PROFILE env var.
    /// Set via `lean-ctx config set profile passthrough` or editing config.toml.
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub loop_detection: LoopDetectionConfig,
    /// Controls where lean-ctx installs agent rule files.
    /// Values: "both" (default), "global" (home-dir only), "project" (repo-local only).
    /// Override via LEAN_CTX_RULES_SCOPE env var.
    #[serde(default)]
    pub rules_scope: Option<String>,
    /// Controls how rules are injected for shared-instruction-file agents.
    /// Values: "shared" (default, marker block in CLAUDE.md/CODEBUDDY.md/AGENTS.md/GEMINI.md),
    /// "dedicated" (never touch those files; use each agent's config-driven
    /// auto-load: SessionStart hook / instructions[] / context.fileName, #343), or
    /// "off" (write no rules file at all — for hosts that supply their own
    /// tool-steering workflow or phase-isolated/non-caching harnesses, #361).
    /// Override via LEAN_CTX_RULES_INJECTION env var.
    #[serde(default)]
    pub rules_injection: Option<String>,
    /// Mirror the host IDE's tool-permission rules onto lean-ctx's own MCP tools.
    /// Values: "off" (default) or "on". When "on", lean-ctx reads the active
    /// IDE's permission config (v1: OpenCode) and applies the equivalent
    /// deny/ask/allow decision to the matching lean-ctx tool — so `ctx_shell`
    /// honors your `bash`/`rm *` rules instead of bypassing them.
    /// Override via LEAN_CTX_PERMISSION_INHERITANCE env var.
    #[serde(default)]
    pub permission_inheritance: Option<String>,
    /// Extra glob patterns to ignore in graph/overview/preload (repo-local).
    /// Example: `["externals/**", "target/**", "temp/**"]`
    #[serde(default)]
    pub extra_ignore_patterns: Vec<String>,
    /// Controls agent output verbosity via instructions injection.
    /// Values: "off" (default), "lite", "full", "ultra".
    /// Override via LEAN_CTX_TERSE_AGENT env var.
    #[serde(default)]
    pub terse_agent: TerseAgent,
    /// Unified compression level (replaces separate terse_agent + output_density).
    /// Values: "off" (default), "lite", "standard", "max".
    /// Override via LEAN_CTX_COMPRESSION env var.
    #[serde(default)]
    pub compression_level: CompressionLevel,
    /// Global compression intensity 0.0 (lossless) – 1.0 (max), mapped onto the
    /// read modes / entropy / IB stages (see `core::aggressiveness`). `None`
    /// (default) keeps each mode's built-in default. Override via the
    /// `LEAN_CTX_AGGRESSIVENESS` env var or the `ctx_read` `aggressiveness` arg.
    #[serde(default)]
    pub compression_aggressiveness: Option<f64>,
    /// Archive configuration for zero-loss compression.
    #[serde(default)]
    pub archive: ArchiveConfig,
    /// Memory policy (knowledge/episodic/procedural/lifecycle budgets & thresholds).
    #[serde(default)]
    pub memory: MemoryPolicy,
    /// Additional paths allowed by PathJail (absolute).
    /// Useful for multi-project workspaces where the jail root is a parent directory.
    /// Override via LEAN_CTX_ALLOW_PATH env var (path-list separator).
    #[serde(default)]
    pub allow_paths: Vec<String>,
    /// Allow jailed tool access to home-level IDE config dirs (~/.cursor, VS Code,
    /// Cline/Roo, JetBrains, …). Tri-state: `None` = not asked yet (setup prompts
    /// once), `Some(false)` = declined, `Some(true)` = opted in. Those dirs can
    /// expose other agents' sessions, MCP configs and credentials, so the effective
    /// default is off. `~/.lean-ctx` (own data dir) is always allowed. The opt-in
    /// set is registry-derived, covering every supported editor. Override via
    /// LEAN_CTX_ALLOW_IDE_DIRS=1.
    #[serde(default)]
    pub allow_ide_config_dirs: Option<bool>,
    /// Extra project roots for multi-root workspaces.
    /// Tools like ctx_tree and ctx_search can scan across all roots in a single call.
    /// These paths are automatically added to PathJail's allow-list.
    /// Override via LEAN_CTX_EXTRA_ROOTS env var (path-list separator).
    #[serde(default)]
    pub extra_roots: Vec<String>,
    /// Read-only roots: sibling subtrees the agent may READ but never WRITE.
    /// Reads resolve as if they were extra_roots; every write tool (edit, refactor,
    /// handoff/session export, memory compaction) is default-denied inside these
    /// paths. Useful for reference repos mounted next to the project.
    /// Override via LEAN_CTX_READ_ONLY_ROOTS env var (path-list separator).
    #[serde(default)]
    pub read_only_roots: Vec<String>,
    /// Extra trusted roots OUTSIDE `$HOME` that lean-ctx may follow when an agent
    /// config file/dir (`~/.claude.json`, `~/.codex/config.toml`, …) is a symlink
    /// pointing there (#596). Empty by default → the strict `$HOME`-only boundary
    /// stays in force (a planted symlink can never redirect a config write out of
    /// the user's home, preserving the GL#442 symlink-hijack protection). Add a
    /// parent like `/opt/dotfiles` only for a location you own and trust. Like
    /// `extra_roots`, security-sensitive: stripped from untrusted project-local
    /// configs. Override via LEAN_CTX_ALLOW_SYMLINK_ROOTS env var (path-list sep).
    #[serde(default)]
    pub allow_symlink_roots: Vec<String>,
    /// Enable content-defined chunking (Rabin-Karp) for cache-optimal output ordering.
    /// Stable chunks are emitted first to maximize prompt cache hits.
    #[serde(default)]
    pub content_defined_chunking: bool,
    /// Skip session/knowledge/gotcha blocks in MCP instructions to minimize token overhead.
    /// Override via LEAN_CTX_MINIMAL env var.
    ///
    /// Default `true` (deliberate): initialize-time instructions stay byte-stable
    /// across sessions, which keeps the provider prompt-cache prefix warm (#498)
    /// and holds the fixed per-session cost at the `doctor overhead --gate`
    /// budget. Session continuity is NOT lost — the wakeup briefing (task,
    /// findings, knowledge) is delivered through the `--- AUTO CONTEXT ---`
    /// block of the first tool call other than the `ctx_compose` overview,
    /// which only bills when the agent actually works. Set to `false` to additionally inject the ACTIVE SESSION
    /// / PROJECT MEMORY blocks directly into the MCP `initialize` instructions.
    #[serde(default)]
    pub minimal_overhead: bool,
    /// Opt-in: substitute long identifiers with short α-codes (+ a `§MAP` table)
    /// in `aggressive` reads for projects with >50 source files. Off by default —
    /// the abbreviated form is confusing for editing/refactoring, where the agent
    /// needs the real package and symbol names. Enable for max exploration savings.
    #[serde(default)]
    pub symbol_map_auto: bool,
    /// Opt-in: let the adaptive *learning* signals (predictor, bandit, heatmap,
    /// adaptive policy, bounce/path memory) participate in `auto` mode
    /// resolution. Off by default (#683): the default cascade is a deterministic
    /// function of (file, task) — only capability guards and the size/task
    /// heuristic decide — which keeps output byte-stable for provider prompt
    /// caching (#498) and avoids per-read disk I/O from the learning stores.
    /// Override via the LEAN_CTX_AUTO_MODE_LEARNING env var.
    #[serde(default)]
    pub auto_mode_learning: bool,
    /// Opt-in: auto-persist interesting findings as knowledge facts.
    #[serde(default)]
    pub auto_capture: bool,
    /// Hybrid search weights (BM25/dense/candidates).
    #[serde(default)]
    pub search: crate::core::hybrid_search::HybridConfig,
    /// Code-graph settings, including traversal (co-access) edges (#289).
    #[serde(default)]
    pub graph: GraphConfig,
    /// Index-time file filters (#735): include/exclude globs + gitignore
    /// handling, applied by every index builder via `core::index_filter`.
    #[serde(default)]
    pub index: IndexConfig,
    /// AI session-summary settings (#292): periodic, semantically-recallable summaries.
    #[serde(default)]
    pub summaries: SummariesConfig,
    /// Optional LLM enhancement (query expansion, contradiction explanation).
    #[serde(default)]
    pub llm: crate::core::llm_enhance::LlmConfig,
    /// Semantic-embedding engine settings (which local ONNX model to use).
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    /// Disable shell hook injection (the _lc() function that wraps CLI commands).
    /// Override via LEAN_CTX_NO_HOOK env var.
    #[serde(default)]
    pub shell_hook_disabled: bool,
    /// Shadow mode (default: true): denies native tools (Read/Grep/Shell) at
    /// the permission level, forcing agents to use ctx_* MCP tools for maximum
    /// compression. Without this, many harnesses silently prefer native tools,
    /// negating lean-ctx's token savings. Disable with `shadow_mode = false`.
    #[serde(default = "serde_defaults::default_true")]
    pub shadow_mode: bool,
    /// Global hook mode override. When set, overrides the per-agent auto-detection.
    /// - `replace`: Native Read/Grep/Glob/Shell denied, lean-ctx MCP is the only path
    /// - `hybrid`: MCP + shell hooks for compression (legacy)
    /// - `mcp`: MCP server only, no hooks
    ///
    /// Default: unset (auto-detect per agent via `recommend_hook_mode`)
    #[serde(default)]
    pub hook_mode: Option<String>,
    /// Opt-in (#520): write a human-readable debug log of intercepted MCP tool
    /// calls and hook routing decisions (lean-ctx vs native, with reasons) to
    /// `<state_dir>/logs/debug.log`. Override via the LEAN_CTX_DEBUG_LOG env var.
    #[serde(default)]
    pub debug_log: bool,
    /// Controls when the shell hook auto-activates aliases.
    /// - `agents-only`: (Default since #699) Aliases only active when an AI
    ///   agent env var is detected — transparent in plain human terminals.
    /// - `always`: Aliases active in every interactive shell (pre-#699 default).
    /// - `off`: Aliases never auto-activate (user must call `lean-ctx-on` manually).
    ///
    /// Override via `LEAN_CTX_SHELL_ACTIVATION` env var.
    #[serde(default)]
    pub shell_activation: ShellActivation,
    /// Do not install agent CLI aliases (`claude`, `codex`, `gemini`,
    /// `codebuddy`) into `~/.zshrc` / `~/.bashrc` during `onboard` / `setup`.
    /// Existing alias blocks are removed when this is toggled on (#754).
    /// Does NOT affect the shell compression hook (`_lc()`) — use
    /// `shell_hook_disabled` for that. Orthogonal to `shell_activation` which
    /// controls *when* aliases activate, not *whether* they are installed.
    #[serde(default)]
    pub skip_agent_aliases: bool,
    /// Controls the native-Read → `ctx_read` redirect hook (#637).
    /// - `auto`: (Default) redirect everywhere except hosts with a native
    ///   read-before-write guard (Claude Code / CodeBuddy), where the path-swap
    ///   would break native Write/Edit.
    /// - `on`: always redirect (legacy behavior).
    /// - `off`: never redirect native Read.
    ///
    /// Override via the `LEAN_CTX_READ_REDIRECT` env var.
    #[serde(default)]
    pub read_redirect: ReadRedirect,
    /// Controls the PostToolUse native-Read re-read dedup (GL #1140).
    /// - `auto`: (Default) replace only re-reads of unchanged files, and only on
    ///   guard hosts (Claude Code / CodeBuddy) where the PreToolUse redirect is
    ///   disabled — the guard-safe way to win the dedup savings back.
    /// - `on`: dedup wherever the PostToolUse hook fires.
    /// - `off`: never replace a Read result.
    ///
    /// Override via the `LEAN_CTX_READ_DEDUP` env var.
    #[serde(default)]
    pub read_dedup: ReadDedup,
    #[serde(default)]
    pub updates: UpdatesConfig,
    /// Fixed-context budget accounting for `doctor overhead` / `gain` (#964).
    #[serde(default)]
    pub context: ContextConfig,
    /// Maximum BM25 cache file size in MB. Indexes exceeding this are quarantined on load
    /// and refused on save. Override via LEAN_CTX_BM25_MAX_CACHE_MB env var.
    #[serde(default = "serde_defaults::default_bm25_max_cache_mb")]
    pub bm25_max_cache_mb: u64,
    /// Maximum number of files scanned by the lightweight JSON graph index.
    /// 0 = unlimited (default). Set >0 to cap for constrained systems.
    #[serde(default = "serde_defaults::default_graph_index_max_files")]
    pub graph_index_max_files: u64,
    /// Controls RAM vs feature trade-off. Values: "low", "balanced" (default), "performance".
    /// Override via LEAN_CTX_MEMORY_PROFILE env var.
    #[serde(default)]
    pub memory_profile: MemoryProfile,
    /// Controls how aggressively memory is freed when idle.
    /// Values: "shared" (default, 1h TTL), "aggressive" (5 min TTL for low-memory devices).
    /// Override via LEAN_CTX_MEMORY_CLEANUP env var.
    #[serde(default)]
    pub memory_cleanup: MemoryCleanup,
    /// Soft process-RSS target as a percentage of system RAM (default: 5).
    /// The guardian throttles and evicts above it, but this is not an OS hard cap.
    /// Use a cgroup/container MemoryMax when strict isolation is required.
    /// Override via LEAN_CTX_MAX_RAM_PERCENT env var.
    #[serde(default = "serde_defaults::default_max_ram_percent")]
    pub max_ram_percent: u8,
    /// Simplified disk budget (MB). When set and detail values are at defaults,
    /// distributes proportionally: archive=25%, bm25=10%, remainder for stores.
    /// 0 = disabled (use individual settings). Override via LEAN_CTX_MAX_DISK_MB.
    #[serde(default)]
    pub max_disk_mb: u64,
    /// Auto-purge data older than this many days. 0 = disabled.
    /// Flows into archive.max_age_hours and lifecycle idle TTL.
    #[serde(default)]
    pub max_staleness_days: u32,
    /// Cap on the rayon worker threads used by the CPU-heavy index build
    /// (call graph etc.). 0 = rayon default (all cores). Set >0 to bound
    /// per-instance CPU so a fleet of concurrent sessions can't saturate the
    /// host on startup. Override via LEANCTX_INDEX_THREADS env var.
    #[serde(default)]
    pub max_index_threads: usize,
    /// Controls visibility of token savings footers in tool output.
    /// Values: "always" (default, show on every response), "never", "auto" (legacy compatibility).
    /// Override via LEAN_CTX_SAVINGS_FOOTER or LEAN_CTX_SHOW_SAVINGS=1|0 env var.
    #[serde(default)]
    pub savings_footer: SavingsFooter,
    /// Explicit project root override. When set, lean-ctx uses this instead of auto-detection.
    /// This prevents accidental home-directory scans when running from $HOME.
    /// Override via LEAN_CTX_PROJECT_ROOT env var.
    #[serde(default)]
    pub project_root: Option<String>,
    /// LSP server overrides. Map language name to custom binary path.
    /// Example: `[lsp]\nrust = "/opt/rust-analyzer"\npython = "~/.venvs/main/bin/pylsp"`
    #[serde(default)]
    pub lsp: std::collections::HashMap<String, String>,
    /// Per-IDE allowed paths. Restricts which directories lean-ctx will scan/index for each IDE.
    /// Example: `[ide_paths]\ncursor = ["/home/user/projects/app1"]\ncodex = ["/home/user/codex"]`
    /// When set, only these paths are indexed for the matching agent. Global `allow_paths` still applies.
    #[serde(default)]
    pub ide_paths: HashMap<String, Vec<String>>,
    /// Custom model context window overrides.
    /// Example: `[model_context_windows]\n"my-custom-model" = 500000`
    #[serde(default)]
    pub model_context_windows: HashMap<String, usize>,
    /// Controls how much detail tool responses include.
    ///
    /// - `full` (default): complete compressed output
    /// - `headers_only`: metadata line only (path, mode, token count)
    ///
    /// Override via `LEAN_CTX_RESPONSE_VERBOSITY` env var.
    #[serde(default)]
    pub response_verbosity: ResponseVerbosity,
    /// Bypass hint mode. When agents use native Read/Grep instead of lean-ctx tools,
    /// a hint is appended to the next tool response.
    /// Values: "on" (default), "off", "aggressive" (hint on every call, no cooldown).
    /// Override via LEAN_CTX_BYPASS_HINTS env var.
    #[serde(default)]
    pub bypass_hints: Option<String>,
    /// Cache policy for ctx_read. Controls behavior on cache hits.
    /// Values: "aggressive" (default, 13-tok stubs + compaction-aware reset),
    /// "safe" (delivers map instead of stub), "off" (no caching, always disk read).
    /// Override via LEAN_CTX_CACHE_POLICY env var.
    #[serde(default)]
    pub cache_policy: Option<String>,
    /// Token budget for the in-memory `ctx_read` cache. When the cached total
    /// plus an incoming read would exceed this, lean-ctx evicts the least-valuable
    /// entries *immediately* (RRF: recency × frequency × size) so the read always
    /// proceeds — eviction is never deferred to the staleness TTL. `0` uses the
    /// built-in default (2M). `LEAN_CTX_CACHE_MAX_TOKENS` env var overrides this.
    #[serde(default)]
    pub cache_max_tokens: usize,
    /// Cross-project boundary policy.
    /// Controls whether cross-project search/import is allowed and whether access is audited.
    #[serde(default)]
    pub boundary_policy: crate::core::memory_boundary::BoundaryPolicy,
    #[serde(default)]
    pub secret_detection: SecretDetectionConfig,
    /// Per-item sensitivity model with a uniform policy floor (#212).
    /// Disabled by default → fully no-op until `sensitivity.enabled = true`.
    #[serde(default)]
    pub sensitivity: crate::core::sensitivity::SensitivityConfig,
    /// Addon ecosystem security floor (#863): install policy, registry-signature
    /// requirement and sandboxing for spawned addon servers. Global-only (never
    /// merged from project-local config) and fully permissive by default.
    #[serde(default)]
    pub addons: crate::core::artifacts_exec::policy::AddonsConfig,
    /// Allow automatic project-root re-rooting when absolute paths outside the jail are seen.
    /// When false (default), absolute paths outside the jail are rejected without re-rooting.
    /// Override via LEAN_CTX_ALLOW_REROOT env var.
    #[serde(default)]
    pub allow_auto_reroot: bool,
    /// Verbatim binary path/expression for generated agent-hook commands
    /// (#708). Users who sync agent settings (`~/.claude/settings.json`, …)
    /// across machines with different usernames need an env-based form like
    /// `$HOME/.local/bin/lean-ctx` — agent hosts run hook commands through a
    /// shell, which expands it. When set (env `LEAN_CTX_HOOK_BINARY` wins,
    /// then this key), every hook writer emits the value verbatim instead of
    /// the machine-absolute exe path, so `init`/`doctor --fix`/`update` stop
    /// rewriting synced files into sync ping-pong. Autostart plists/services
    /// and daemon spawns are NOT affected — launchd/systemd do not expand
    /// shell variables, so those keep the real absolute path. Empty (default)
    /// = automatic absolute-path resolution (#367).
    #[serde(default)]
    pub hook_binary: Option<String>,
    /// Disable PathJail entirely by setting `path_jail = false` in config.toml.
    /// Useful in container/Docker environments where the sandbox is the boundary.
    /// (The former `LEAN_CTX_NO_JAIL=1` env override was removed in v3.7.3.)
    #[serde(default)]
    pub path_jail: Option<bool>,
    /// Sandbox level for code execution (ctx_exec).
    /// 0 = subprocess only (current), 1 = OS-level restriction (Seatbelt/Landlock).
    /// Override via LEAN_CTX_SANDBOX_LEVEL env var.
    #[serde(default)]
    pub sandbox_level: u8,
    /// When true, large tool outputs (>4000 chars) are stored as references
    /// and a short URI is returned instead of the full content.
    /// Override via LEAN_CTX_REFERENCE_RESULTS env var.
    #[serde(default)]
    pub reference_results: bool,
    /// Default per-agent token budget. 0 means unlimited.
    /// Override per-agent via ctx_session or programmatically.
    #[serde(default)]
    pub agent_token_budget: usize,

    /// Shell-security mode for ctx_shell / `lean-ctx -c` command gating (GL #788):
    /// `enforce` (default, secure), `warn` (run checks, log violations, never
    /// block) or `off` (skip the hard blocks entirely —
    /// a deliberate opt-out; compression stays active). Override via
    /// LEAN_CTX_SHELL_SECURITY. `None` resolves to `enforce`.
    #[serde(default)]
    pub shell_security: Option<String>,

    /// Default shell-command timeout in seconds for *normal* commands. `None`
    /// resolves to the built-in 2-minute default; heavy builds/tests use
    /// [`Config::shell_heavy_timeout_secs`]. Override via
    /// `LEAN_CTX_SHELL_TIMEOUT_SECS` (`LEAN_CTX_SHELL_TIMEOUT_MS` still wins over
    /// both, in milliseconds).
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,

    /// Shell-command timeout in seconds for *heavy* commands (cargo build/test,
    /// make, docker build, git commit/push, …). `None` resolves to the built-in
    /// 10-minute ceiling. Override via `LEAN_CTX_SHELL_HEAVY_TIMEOUT_SECS`.
    #[serde(default)]
    pub shell_heavy_timeout_secs: Option<u64>,

    /// Extra command prefixes that get the heavy timeout ceiling. Merged with
    /// the built-in list. Useful for project-specific long-running scripts.
    /// Example: `shell_heavy_prefixes = ["python3 ", "./scripts/"]`
    #[serde(default)]
    pub shell_heavy_prefixes: Vec<String>,
    /// #814: opt-in to allow `python3 -c`, `node -e`, etc. in ctx_shell.
    /// Default `false` — inline code is blocked because it leaves no auditable
    /// artifact. Override via `LEAN_CTX_SHELL_ALLOW_INLINE_SCRIPTS=1`.
    #[serde(default)]
    pub shell_allow_inline_scripts: bool,

    /// Setup behavior: controls what gets injected during setup and updates.
    #[serde(default)]
    pub setup: SetupConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tee_mode: TeeMode::default(),
            recovery_hints: RecoveryHints::default(),
            output_density: OutputDensity::default(),
            checkpoint_interval: 15,
            passthrough_commands: Vec::new(),
            passthrough_urls: Vec::new(),
            preserve_compact_formats: serde_defaults::default_preserve_compact_formats(),
            crush_verbatim_json: false,
            slow_command_threshold_ms: 5000,
            theme: serde_defaults::default_theme(),
            gain: GainConfig::default(),
            cost: CostConfig::default(),
            code_health: CodeHealthConfig::default(),
            autonomy: AutonomyConfig::default(),
            providers: ProvidersConfig::default(),
            proxy: ProxyConfig::default(),
            conversation: ConversationConfig::default(),
            response_shaping: ResponseShapingConfig::default(),
            proxy_enabled: None,
            proxy_port: None,
            proxy_timeout_ms: None,
            proxy_require_token: false,
            proxy_loopback_open: false,
            dashboard_cache_hit_rate: None,
            enable_wakeup_ctx: true,
            prefer_native_editor: false,
            default_tool_categories: Vec::new(),
            no_degrade: false,
            delta_explicit: true,
            profile: None,
            loop_detection: LoopDetectionConfig::default(),
            rules_scope: None,
            rules_injection: None,
            permission_inheritance: None,
            extra_ignore_patterns: Vec::new(),
            terse_agent: TerseAgent::default(),
            compression_level: CompressionLevel::default(),
            compression_aggressiveness: None,
            archive: ArchiveConfig::default(),
            memory: MemoryPolicy::default(),
            allow_paths: Vec::new(),
            allow_ide_config_dirs: None,
            extra_roots: Vec::new(),
            read_only_roots: Vec::new(),
            allow_symlink_roots: Vec::new(),
            content_defined_chunking: false,
            minimal_overhead: true,
            symbol_map_auto: false,
            auto_mode_learning: false,
            auto_capture: true,
            search: crate::core::hybrid_search::HybridConfig::default(),
            graph: GraphConfig::default(),
            index: IndexConfig::default(),
            summaries: SummariesConfig::default(),
            llm: crate::core::llm_enhance::LlmConfig::default(),
            embedding: EmbeddingConfig::default(),
            shell_hook_disabled: false,
            shadow_mode: true,
            hook_mode: None,
            debug_log: false,
            shell_activation: ShellActivation::default(),
            skip_agent_aliases: false,
            read_redirect: ReadRedirect::default(),
            read_dedup: ReadDedup::default(),
            updates: UpdatesConfig::default(),
            context: ContextConfig::default(),
            graph_index_max_files: serde_defaults::default_graph_index_max_files(),
            bm25_max_cache_mb: serde_defaults::default_bm25_max_cache_mb(),
            memory_profile: MemoryProfile::default(),
            memory_cleanup: MemoryCleanup::default(),
            max_ram_percent: serde_defaults::default_max_ram_percent(),
            max_disk_mb: 0,
            max_staleness_days: 0,
            max_index_threads: 0,
            savings_footer: SavingsFooter::default(),
            project_root: None,
            lsp: std::collections::HashMap::new(),
            ide_paths: HashMap::new(),
            model_context_windows: HashMap::new(),
            response_verbosity: ResponseVerbosity::default(),
            bypass_hints: None,
            cache_policy: None,
            cache_max_tokens: 0,
            boundary_policy: crate::core::memory_boundary::BoundaryPolicy::default(),
            secret_detection: SecretDetectionConfig::default(),
            sensitivity: crate::core::sensitivity::SensitivityConfig::default(),
            addons: crate::core::artifacts_exec::policy::AddonsConfig::default(),
            allow_auto_reroot: false,
            hook_binary: None,
            path_jail: None,
            sandbox_level: 0,
            reference_results: false,
            agent_token_budget: 0,
            shell_security: None,
            shell_timeout_secs: None,
            shell_heavy_timeout_secs: None,
            shell_heavy_prefixes: Vec::new(),
            shell_allow_inline_scripts: false,
            setup: SetupConfig::default(),
        }
    }
}

impl Config {
    /// Whether opt-in lossless JSON crushing of verbatim data commands (#936) is
    /// active. `LEAN_CTX_CRUSH_VERBATIM_JSON` (any value) wins, then the
    /// `crush_verbatim_json` config flag, else `false`.
    pub fn crush_verbatim_json_effective(&self) -> bool {
        std::env::var("LEAN_CTX_CRUSH_VERBATIM_JSON").is_ok() || self.crush_verbatim_json
    }

    /// Returns the effective rules scope, preferring env var over config file.
    pub fn rules_scope_effective(&self) -> RulesScope {
        let raw = std::env::var("LEAN_CTX_RULES_SCOPE")
            .ok()
            .or_else(|| self.rules_scope.clone())
            .unwrap_or_default();
        match raw.trim().to_lowercase().as_str() {
            "global" => RulesScope::Global,
            "project" => RulesScope::Project,
            _ => RulesScope::Both,
        }
    }

    /// Returns the effective rules injection mode, preferring env var over config.
    /// Default is `Shared` (zero-config discovery via a CLAUDE.md/CODEBUDDY.md/AGENTS.md block).
    pub fn rules_injection_effective(&self) -> RulesInjection {
        let raw = std::env::var("LEAN_CTX_RULES_INJECTION")
            .ok()
            .or_else(|| self.rules_injection.clone())
            .unwrap_or_default();
        match raw.trim().to_lowercase().as_str() {
            "dedicated" => RulesInjection::Dedicated,
            "off" | "none" | "disabled" => RulesInjection::Off,
            _ => RulesInjection::Shared,
        }
    }

    /// Provider prompt-cache hit rate for net-of-injection (#1104).
    /// Returns the configured value or None (caller picks the default).
    #[must_use]
    pub fn dashboard_cache_hit_rate(&self) -> Option<f64> {
        std::env::var("LEAN_CTX_CACHE_HIT_RATE")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(self.dashboard_cache_hit_rate)
    }
    /// Returns the user-configured hook mode override, or `None` for auto-detect.
    /// Env var `LEAN_CTX_HOOK_MODE` takes priority over config.
    #[must_use]
    pub fn hook_mode_override(&self) -> Option<crate::hooks::HookMode> {
        let raw = std::env::var("LEAN_CTX_HOOK_MODE")
            .ok()
            .or_else(|| self.hook_mode.clone())?;
        crate::hooks::HookMode::from_str_loose(raw.trim())
    }

    /// Returns the effective permission-inheritance mode, preferring the
    /// `LEAN_CTX_PERMISSION_INHERITANCE` env var over config. Default is `Off`.
    /// Accepts `on`/`true`/`1` as enabled.
    #[must_use]
    pub fn permission_inheritance_effective(&self) -> PermissionInheritance {
        let raw = std::env::var("LEAN_CTX_PERMISSION_INHERITANCE")
            .ok()
            .or_else(|| self.permission_inheritance.clone())
            .unwrap_or_default();
        match raw.trim().to_lowercase().as_str() {
            "on" | "true" | "1" | "inherit" => PermissionInheritance::On,
            _ => PermissionInheritance::Off,
        }
    }

    /// True when lean-ctx should inject its rules via each agent's dedicated,
    /// non-polluting auto-load path *and* global rules are in scope.
    ///
    /// Gates the Claude/Codex `SessionStart` `additionalContext` summary: it
    /// stands in for the (now-skipped) shared CLAUDE.md/CODEBUDDY.md/AGENTS.md block, so it
    /// only fires when injection is `Dedicated` and the scope isn't project-only.
    #[must_use]
    pub fn dedicated_session_context_active(&self) -> bool {
        self.rules_injection_effective() == RulesInjection::Dedicated
            && self.rules_scope_effective() != RulesScope::Project
    }

    /// Whether lean-ctx edit operations are disabled in favour of the host's
    /// native editor (#454). `LEAN_CTX_PREFER_NATIVE_EDITOR` wins over config.
    pub fn prefer_native_editor_effective(&self) -> bool {
        match std::env::var("LEAN_CTX_PREFER_NATIVE_EDITOR") {
            Ok(raw) => matches!(
                raw.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => self.prefer_native_editor,
        }
    }

    /// Cap on the rayon index-build worker threads. `LEANCTX_INDEX_THREADS` wins
    /// over config; `0` means "no cap" — rayon's all-cores default is kept.
    pub fn max_index_threads_effective(&self) -> usize {
        std::env::var("LEANCTX_INDEX_THREADS")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(self.max_index_threads)
    }

    /// Whether `name` is a lean-ctx edit operation that must be blocked from
    /// dispatch (direct and via `ctx_call`) when [`Self::prefer_native_editor_effective`]
    /// is set (#454). Read/search/shell/memory tools are never blocked.
    pub fn edit_tool_blocked(&self, name: &str) -> bool {
        self.prefer_native_editor_effective() && EDIT_TOOL_NAMES.contains(&name)
    }

    /// Returns `true` if minimal overhead is enabled via env var or config.
    pub fn minimal_overhead_effective(&self) -> bool {
        std::env::var("LEAN_CTX_MINIMAL").is_ok() || self.minimal_overhead
    }

    /// Returns `true` when the adaptive learning signals may participate in
    /// `auto` mode resolution (#683). Off by default for a deterministic,
    /// I/O-light cascade; the `LEAN_CTX_AUTO_MODE_LEARNING` env var wins over the
    /// config field and accepts the usual truthy/falsy spellings.
    pub fn auto_mode_learning_effective(&self) -> bool {
        match std::env::var("LEAN_CTX_AUTO_MODE_LEARNING") {
            Ok(raw) => matches!(
                raw.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => self.auto_mode_learning,
        }
    }

    /// Returns `true` when probabilistic exploration (Thompson sampling,
    /// Boltzmann-temperature eviction, simulated annealing) may influence
    /// decisions. Off by default so tool output stays a deterministic, byte-
    /// stable function of (content, mode, task) — the determinism contract
    /// (#498) that lets provider prompt caching apply. The `LEAN_CTX_STOCHASTIC`
    /// env var wins (the usual truthy/falsy spellings); otherwise it follows
    /// [`Self::auto_mode_learning_effective`], which is itself off by default.
    pub fn is_stochastic_enabled(&self) -> bool {
        match std::env::var("LEAN_CTX_STOCHASTIC") {
            Ok(raw) => matches!(
                raw.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => self.auto_mode_learning_effective(),
        }
    }

    /// Returns `true` if minimal overhead should be enabled for this MCP client.
    ///
    /// This is a superset of `minimal_overhead_effective()`:
    /// - `LEAN_CTX_OVERHEAD_MODE=minimal` forces minimal overhead
    /// - `LEAN_CTX_OVERHEAD_MODE=full` disables client/model heuristics (still honors LEAN_CTX_MINIMAL / config)
    /// - In auto mode (default), certain low-context clients/models are treated as minimal to prevent
    ///   large metadata blocks from destabilizing smaller context windows (e.g. Hermes + MiniMax).
    pub fn minimal_overhead_effective_for_client(&self, client_name: &str) -> bool {
        if let Ok(raw) = std::env::var("LEAN_CTX_OVERHEAD_MODE") {
            match raw.trim().to_lowercase().as_str() {
                "minimal" => return true,
                "full" => return self.minimal_overhead_effective(),
                _ => {}
            }
        }

        if self.minimal_overhead_effective() {
            return true;
        }

        let client_lower = client_name.trim().to_lowercase();
        if !client_lower.is_empty() {
            if let Ok(list) = std::env::var("LEAN_CTX_MINIMAL_CLIENTS") {
                for needle in list.split(',').map(|s| s.trim().to_lowercase()) {
                    if !needle.is_empty() && client_lower.contains(&needle) {
                        return true;
                    }
                }
            } else if client_lower.contains("hermes") || client_lower.contains("minimax") {
                return true;
            }
        }

        let model = std::env::var("LEAN_CTX_MODEL")
            .or_else(|_| std::env::var("LCTX_MODEL"))
            .unwrap_or_default();
        let model = model.trim().to_lowercase();
        if !model.is_empty() {
            let m = model.replace(['_', ' '], "-");
            if m.contains("minimax")
                || m.contains("mini-max")
                || m.contains("m2.7")
                || m.contains("m2-7")
            {
                return true;
            }
        }

        false
    }

    /// Returns `true` if shell hook injection is disabled via env var or config.
    pub fn shell_hook_disabled_effective(&self) -> bool {
        std::env::var("LEAN_CTX_NO_HOOK").is_ok() || self.shell_hook_disabled
    }

    /// Returns the effective shell activation mode (env var > config > default).
    pub fn shell_activation_effective(&self) -> ShellActivation {
        ShellActivation::effective(self)
    }

    /// #814: returns `true` if `ctx_shell` may accept inline interpreter scripts
    /// (`python3 -c "..."`, `node -e "..."`, etc.).
    /// `LEAN_CTX_SHELL_ALLOW_INLINE_SCRIPTS` (`1`/`true`/`yes`/`on`) overrides
    /// `config.toml`. The hard blocks still apply.
    pub fn shell_allow_inline_scripts_effective(&self) -> bool {
        match std::env::var("LEAN_CTX_SHELL_ALLOW_INLINE_SCRIPTS") {
            Ok(raw) => matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => self.shell_allow_inline_scripts,
        }
    }

    pub fn memory_policy_effective(&self) -> Result<MemoryPolicy, String> {
        let mut policy = self.memory.clone();
        policy.apply_env_overrides();

        let budget = self.max_disk_mb_effective();
        if budget > 0 {
            let scale_factor = (budget as f64 / 500.0).clamp(0.5, 10.0);
            let default_policy = MemoryPolicy::default();
            if policy.knowledge.max_facts == default_policy.knowledge.max_facts {
                policy.knowledge.max_facts = (200.0 * scale_factor) as usize;
            }
            if policy.knowledge.max_patterns == default_policy.knowledge.max_patterns {
                policy.knowledge.max_patterns = (50.0 * scale_factor) as usize;
            }
            if policy.episodic.max_episodes == default_policy.episodic.max_episodes {
                policy.episodic.max_episodes = (500.0 * scale_factor) as usize;
            }
            if policy.procedural.max_procedures == default_policy.procedural.max_procedures {
                policy.procedural.max_procedures = (100.0 * scale_factor) as usize;
            }
        }

        policy.validate()?;
        Ok(policy)
    }

    /// Returns the effective set of default tool categories.
    /// Priority: LCTX_DEFAULT_CATEGORIES env var > config.toml > hardcoded default.
    pub fn default_tool_categories_effective(&self) -> Vec<String> {
        if let Ok(val) = std::env::var("LCTX_DEFAULT_CATEGORIES") {
            return val
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if !self.default_tool_categories.is_empty() {
            return self
                .default_tool_categories
                .iter()
                .map(|s| s.to_lowercase())
                .collect();
        }
        vec!["core".to_string(), "session".to_string()]
    }

    /// The `[sensitivity]` config as written. It used to be folded with a
    /// per-persona floor; the personas that declared one are gone.
    #[must_use]
    pub fn sensitivity_effective(&self) -> crate::core::sensitivity::SensitivityConfig {
        self.sensitivity.clone()
    }

    /// Returns `true` if all automatic read-mode degradation is disabled.
    /// Checks LCTX_NO_DEGRADE env var first, then config.toml field.
    pub fn no_degrade_effective(&self) -> bool {
        if let Ok(val) = std::env::var("LCTX_NO_DEGRADE") {
            return val == "1" || val.eq_ignore_ascii_case("true");
        }
        self.no_degrade
    }

    /// Returns `true` if explicit `full`/`lines:N-M` re-reads of
    /// cached-but-changed files should be served as deltas (`mode=diff`)
    /// instead of re-emitting full content.
    ///
    /// Checks the `LCTX_DELTA_EXPLICIT` env var first, then the config.toml
    /// field. Unlike a presence-only knob, an explicit `0`/`false` in the env
    /// forces the feature OFF even when the config field is `true`, so the env
    /// can fully override config in both directions.
    pub fn delta_explicit_effective(&self) -> bool {
        if let Ok(val) = std::env::var("LCTX_DELTA_EXPLICIT") {
            return val == "1" || val.eq_ignore_ascii_case("true");
        }
        self.delta_explicit
    }

    /// Effective max_disk_mb from env or config.
    pub fn max_disk_mb_effective(&self) -> u64 {
        std::env::var("LEAN_CTX_MAX_DISK_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.max_disk_mb)
    }

    /// Effective max_staleness_days from env or config.
    pub fn max_staleness_days_effective(&self) -> u32 {
        std::env::var("LEAN_CTX_MAX_STALENESS_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.max_staleness_days)
    }

    /// Effective fixed-context budget (tokens) from env or config (#964). `0`
    /// (env or config) disables the warning; otherwise the per-session footprint
    /// is checked against this in `doctor overhead` and `gain`.
    pub fn context_budget_tokens_effective(&self) -> usize {
        std::env::var("LEAN_CTX_CONTEXT_BUDGET_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.context.budget_tokens)
    }

    /// Whether later tool calls may receive matching CCR context.
    pub fn proactive_expansion_effective(&self) -> bool {
        self.context.proactive_expansion
    }

    /// Per-response token budget for proactive CCR expansion.
    pub fn proactive_expansion_budget_tokens_effective(&self) -> usize {
        self.context.proactive_expansion_budget_tokens
    }

    /// Normalized BM25 score threshold for proactive CCR expansion.
    pub fn proactive_expansion_threshold_effective(&self) -> f64 {
        self.context.proactive_expansion_threshold
    }

    /// Maximum archive age considered for proactive expansion.
    pub fn proactive_expansion_max_age_secs_effective(&self) -> u64 {
        self.context.proactive_expansion_max_age_secs
    }

    /// Archive max_disk_mb derived from simplified max_disk_mb if the detail
    /// value is still at its default. Explicit overrides take priority.
    pub fn archive_max_disk_mb_effective(&self) -> u64 {
        let budget = self.max_disk_mb_effective();
        if budget > 0 && self.archive.max_disk_mb == ArchiveConfig::default().max_disk_mb {
            budget * 25 / 100
        } else {
            self.archive.max_disk_mb
        }
    }

    /// Archive max_age_hours derived from max_staleness_days if the detail
    /// value is still at its default. Explicit overrides take priority.
    pub fn archive_max_age_hours_effective(&self) -> u64 {
        let staleness = self.max_staleness_days_effective();
        if staleness > 0 && self.archive.max_age_hours == ArchiveConfig::default().max_age_hours {
            staleness as u64 * 24
        } else {
            self.archive.max_age_hours
        }
    }

    /// Effective on-disk ceiling (MB) for the persisted BM25 index. Single source
    /// of truth for `save`/`load`, `cache prune`, and the doctor health check.
    ///
    /// Priority: explicit `bm25_max_cache_mb` › `max_disk_mb` budget (10%) ›
    /// generous default ([`DEFAULT_BM25_PERSIST_MB`]). The default is decoupled
    /// from the RAM profile so large repos persist instead of rebuilding forever
    /// (issue #249).
    pub fn bm25_max_cache_mb_effective(&self) -> u64 {
        if self.bm25_max_cache_mb != serde_defaults::default_bm25_max_cache_mb() {
            return self.bm25_max_cache_mb;
        }
        let budget = self.max_disk_mb_effective();
        if budget > 0 {
            return budget * 10 / 100;
        }
        DEFAULT_BM25_PERSIST_MB
    }
}

impl Config {
    /// Safely mutate and persist the GLOBAL config. Reads the global file only
    /// (no project-local merge), applies `f`, then writes minimally. Refuses
    /// (returns `Err`) when the file exists but is unparseable, so a typo can
    /// never clobber a customized config (#443). Returns the saved `Config`.
    ///
    /// This is the canonical persistence entry point: prefer it over
    /// `Config::load()` followed by `save()`, which leaks project-local
    /// overrides into the global file.
    pub fn update_global<F>(f: F) -> std::result::Result<Self, super::error::LeanCtxError>
    where
        F: FnOnce(&mut Self),
    {
        let path = Self::path().ok_or_else(|| {
            super::error::LeanCtxError::Config("cannot determine home directory".into())
        })?;
        Self::update_global_at(&path, f)
    }

    /// Path-parameterized core of [`Config::update_global`] (unit-testable).
    fn update_global_at<F>(
        path: &Path,
        f: F,
    ) -> std::result::Result<Self, super::error::LeanCtxError>
    where
        F: FnOnce(&mut Self),
    {
        let mut cfg = match std::fs::read_to_string(path) {
            Ok(raw) if !raw.trim().is_empty() => toml::from_str::<Self>(&raw).map_err(|e| {
                super::error::LeanCtxError::Config(
                    format!(
                        "refusing to modify an unparseable config.toml ({e}); fix it \
                     manually or run `lean-ctx doctor --fix`, then retry"
                    )
                    .into(),
                )
            })?,
            _ => Self::default(),
        };
        f(&mut cfg);
        cfg.save_to(path)?;
        Ok(cfg)
    }

    /// Persists the current config to the global config file.
    ///
    /// Preserves user comments, formatting, and unknown keys, keeps the file
    /// minimal (defaults that were never set on disk stay implicit), and writes
    /// atomically with a `.bak` backup so customizations are always recoverable.
    pub fn save(&self) -> std::result::Result<(), super::error::LeanCtxError> {
        let path = Self::path().ok_or_else(|| {
            super::error::LeanCtxError::Config("cannot determine home directory".into())
        })?;
        self.save_to(&path)
    }

    /// Path-parameterized core of [`Config::save`] (unit-testable).
    fn save_to(&self, path: &Path) -> std::result::Result<(), super::error::LeanCtxError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| super::error::LeanCtxError::Config(e.to_string().into()))?;
        // Baseline = what loading an empty config yields. This honors serde's
        // field-level `#[serde(default)]` (which can diverge from the struct's
        // `Default` impl), so minimal mode skips exactly the keys that a fresh
        // load would produce — no spurious lines on save.
        let baseline = toml::from_str::<Self>("").unwrap_or_else(|_| Self::default());
        let defaults = toml::to_string_pretty(&baseline)
            .map_err(|e| super::error::LeanCtxError::Config(e.to_string().into()))?;
        crate::config_io::write_toml_preserving_minimal(path, &content, &defaults)
            .map_err(|e| super::error::LeanCtxError::Config(e.into()))?;
        Ok(())
    }

    /// Formats the current config as a human-readable string with file paths.
    pub fn show(&self) -> String {
        let global_path = Self::path().map_or_else(
            || "~/.lean-ctx/config.toml".to_string(),
            |p| p.to_string_lossy().to_string(),
        );
        let content = toml::to_string_pretty(self).unwrap_or_default();
        let mut out = format!("Global config: {global_path}\n\n{content}");

        if let Some(root) = Self::find_project_root() {
            let local = Self::local_path(&root);
            if local.exists() {
                out.push_str(&format!("\n\nLocal config (merged): {}\n", local.display()));
            } else {
                out.push_str(&format!(
                    "\n\nLocal config: not found (create {} to override per-project)\n",
                    local.display()
                ));
            }
        }
        out
    }
}
