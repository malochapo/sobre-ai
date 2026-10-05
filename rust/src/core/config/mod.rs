use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::memory_policy::MemoryPolicy;

/// Cache payload for [`Config::load_arc`]: the shared config alongside the
/// content hash of the file it was built from, so a
/// later load re-reads + re-hashes and only rebuilds on a real change (#406).
type ConfigCacheSlot = Option<(Arc<Config>, Option<String>)>;

mod enums;
mod loader;
mod memory;
mod proxy;
mod read_dedup;
pub(crate) mod read_redirect;
mod render;
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
    CompressionLevel, Effort, OutputDensity, RecoveryHints, ResponseVerbosity, RulesInjection,
    RulesScope, SessionDegrade, TeeMode, TerseAgent,
};
pub use loader::last_config_parse_error;
pub use memory::{MemoryCleanup, MemoryGuardConfig, MemoryProfile, SavingsFooter};
pub use proxy::{
    HistoryMode, ProseRanker, ProseRole, ProxyConfig, ProxyMode, ProxyProvider, ResolvedProvider,
    RoutingRules, Upstreams, WireShape, is_local_proxy_url, normalize_url, normalize_url_opt,
    parse_route_target, upstream_drifted, upstream_ids,
};
pub use read_dedup::ReadDedup;
pub use read_redirect::ReadRedirect;
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

/// Compact output formats preserved verbatim instead of recompressed (#342).
pub const PRESERVE_COMPACT_FORMATS: &[&str] = &["toon"];

/// Cap on files the graph index scans, bounding its RAM (#790).
pub const GRAPH_INDEX_MAX_FILES: u64 = 15_000;

/// Share of system RAM the memory guard allows lean-ctx to use.
pub const MAX_RAM_PERCENT: u8 = 5;

/// Commands slower than this get a timing note in their output.
pub const SLOW_COMMAND_THRESHOLD_MS: u64 = 5000;

// Compile-time regression guard (#249): the default disk ceiling must stay well
// above the old RAM-profile caps (64/128 MB) that starved large repos.
const _: () = assert!(DEFAULT_BM25_PERSIST_MB >= 512);

/// lean-ctx tools that edit the user's source files. When
/// `prefer_native_editor` is set (#454) these are refused at dispatch so the
/// host's native editor handles edits instead:
/// `ctx_patch` (anchored edits, #1008) and `ctx_refactor` (LSP rename, move,
/// inline, safe-delete). `ctx_refactor`'s read-only actions go with it; the
/// same lookups stay available through `ctx_search` and `ctx_callgraph`.
pub const EDIT_TOOL_NAMES: &[&str] = &["ctx_patch", "ctx_refactor"];

/// lean-ctx configuration, loaded from the single `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub passthrough_commands: Vec<String>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Controls where lean-ctx installs agent rule files.
    /// Values: "both" (default), "global" (home-dir only), "project" (repo-local only).
    #[serde(default)]
    pub rules_scope: Option<String>,
    /// Controls how rules are injected for shared-instruction-file agents.
    /// Values: "shared" (default, marker block in CLAUDE.md/CODEBUDDY.md/AGENTS.md/GEMINI.md),
    /// "dedicated" (never touch those files; use each agent's config-driven
    /// auto-load: SessionStart hook / instructions[] / context.fileName, #343), or
    /// "off" (write no rules file at all — for hosts that supply their own
    /// tool-steering workflow or phase-isolated/non-caching harnesses, #361).
    #[serde(default)]
    pub rules_injection: Option<String>,
    /// Compression intensity, `0.0` (none) to `1.0` (densest), clamped. Drives
    /// the read-mode tuning continuously (`core::aggressiveness`) and picks the
    /// prose style by band (`CompressionLevel::from_value`). Absent: sobre's
    /// adaptive defaults with the `lite` prose style.
    #[serde(
        default,
        deserialize_with = "serde_defaults::deserialize_compression_level",
        skip_serializing_if = "Option::is_none"
    )]
    pub compression_level: Option<f64>,
    /// Paths the tools may read and write besides the project root (absolute,
    /// `~`/`$VAR` expanded). Each one is also searched as an extra root of a
    /// multi-root workspace, and accepted as a target for config files written
    /// through symlinks. `["/"]` opens the path jail entirely.
    #[serde(default)]
    pub allow_paths: Vec<String>,
    /// Read-only roots: sibling subtrees the agent may READ but never WRITE.
    /// Reads resolve as if they were in `allow_paths`; every write tool (edit, refactor,
    /// handoff/session export, memory compaction) is default-denied inside these
    /// paths. Useful for reference repos mounted next to the project.
    #[serde(default)]
    pub read_only_roots: Vec<String>,
    /// Opt-in (#520): write a human-readable debug log of intercepted MCP tool
    /// calls and hook routing decisions (lean-ctx vs native, with reasons) to
    /// `<state_dir>/logs/debug.log`.
    #[serde(default)]
    pub debug_log: bool,
    /// Controls when the shell hook auto-activates aliases.
    /// - `agents-only`: (Default since #699) Aliases only active when an AI
    ///   agent env var is detected — transparent in plain human terminals.
    /// - `always`: Aliases active in every interactive shell (pre-#699 default).
    /// - `off`: Aliases never auto-activate (user must call `lean-ctx-on` manually).
    #[serde(default)]
    pub shell_activation: ShellActivation,
    /// Do not install agent CLI aliases (`claude`, `codex`, `gemini`,
    /// `codebuddy`) into `~/.zshrc` / `~/.bashrc` during `onboard` / `setup`.
    /// Existing alias blocks are removed when this is toggled on (#754).
    /// Does NOT affect the shell compression hook (`_lc()`) — use
    /// `shell_hook` for that. Orthogonal to `shell_activation` which
    /// controls *when* aliases activate, not *whether* they are installed.
    #[serde(default)]
    pub skip_agent_aliases: bool,
    /// Verbatim binary path/expression for generated agent-hook commands
    /// (#708). Users who sync agent settings (`~/.claude/settings.json`, …)
    /// across machines with different usernames need an env-based form like
    /// `$HOME/.local/bin/lean-ctx` — agent hosts run hook commands through a
    /// shell, which expands it. When set, every hook writer emits the value verbatim instead of
    /// the machine-absolute exe path, so `init`/`doctor --fix`/`update` stop
    /// rewriting synced files into sync ping-pong. Autostart plists/services
    /// and daemon spawns are NOT affected — launchd/systemd do not expand
    /// shell variables, so those keep the real absolute path. Empty (default)
    /// = automatic absolute-path resolution (#367).
    #[serde(default)]
    pub hook_binary: Option<String>,
    /// Timeout in seconds for shell commands. `None` resolves to the built-in
    /// 2-minute default. Heavy builds and tests, which sobre recognises, always
    /// get at least the 10-minute heavy ceiling.
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,
    /// Install the shell hook that routes common commands through sobre.
    #[serde(default = "serde_defaults::default_true")]
    pub shell_hook: bool,
    /// Globs for files kept out of the index and of project walks, on top of
    /// `.gitignore` (always respected).
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Setup behavior: controls what gets injected during setup and updates.
    #[serde(default)]
    pub setup: SetupConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            passthrough_commands: Vec::new(),
            proxy: ProxyConfig::default(),
            rules_scope: None,
            rules_injection: None,
            compression_level: None,
            allow_paths: Vec::new(),
            read_only_roots: Vec::new(),
            debug_log: false,
            shell_activation: ShellActivation::default(),
            skip_agent_aliases: false,
            hook_binary: None,
            shell_timeout_secs: None,
            shell_hook: true,
            ignore: Vec::new(),
            setup: SetupConfig::default(),
        }
    }
}

impl Config {
    /// Whether opt-in lossless JSON crushing of verbatim data commands (#936) is
    /// active. Off.
    pub fn crush_verbatim_json_effective(&self) -> bool {
        false
    }

    /// Returns the effective rules scope.
    pub fn rules_scope_effective(&self) -> RulesScope {
        let raw = self.rules_scope.clone().unwrap_or_default();
        match raw.trim().to_lowercase().as_str() {
            "global" => RulesScope::Global,
            "project" => RulesScope::Project,
            _ => RulesScope::Both,
        }
    }

    /// Returns the effective rules injection mode. Default is `Shared`
    /// (zero-config discovery via a CLAUDE.md/CODEBUDDY.md/AGENTS.md block).
    pub fn rules_injection_effective(&self) -> RulesInjection {
        let raw = self.rules_injection.clone().unwrap_or_default();
        match raw.trim().to_lowercase().as_str() {
            "dedicated" => RulesInjection::Dedicated,
            "off" | "none" | "disabled" => RulesInjection::Off,
            _ => RulesInjection::Shared,
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

    /// Cap on the rayon index-build worker threads; `0` means "no cap" —
    /// rayon's all-cores default is kept.
    pub fn max_index_threads_effective(&self) -> usize {
        0
    }

    /// Minimal overhead: on. Tool outputs carry no optional metadata blocks.
    pub fn minimal_overhead_effective(&self) -> bool {
        true
    }

    /// Whether the adaptive learning signals may participate in `auto` mode
    /// resolution (#683). Off: a deterministic, I/O-light cascade.
    pub fn auto_mode_learning_effective(&self) -> bool {
        false
    }

    /// Whether probabilistic exploration (Thompson sampling, Boltzmann-
    /// temperature eviction, simulated annealing) may influence decisions.
    /// Off, so tool output stays a deterministic, byte-stable function of
    /// (content, mode, task) — the determinism contract (#498) that lets
    /// provider prompt caching apply.
    pub fn is_stochastic_enabled(&self) -> bool {
        self.auto_mode_learning_effective()
    }

    /// Whether minimal overhead applies to this MCP client: always, see
    /// [`Self::minimal_overhead_effective`].
    pub fn minimal_overhead_effective_for_client(&self, _client_name: &str) -> bool {
        self.minimal_overhead_effective()
    }

    /// Returns `true` if shell hook injection is disabled.
    pub fn shell_hook_disabled_effective(&self) -> bool {
        !self.shell_hook
    }

    /// Whether `allow_paths` opens the path jail entirely (`"/"`).
    #[must_use]
    pub fn jail_open(&self) -> bool {
        self.allow_paths.iter().any(|p| p.trim() == "/")
    }

    /// Returns the effective shell activation mode.
    pub fn shell_activation_effective(&self) -> ShellActivation {
        ShellActivation::effective(self)
    }

    /// #814: whether `ctx_shell` may accept inline interpreter scripts
    /// (`python3 -c "..."`, `node -e "..."`, etc.). No.
    pub fn shell_allow_inline_scripts_effective(&self) -> bool {
        false
    }

    /// The memory policy: [`MemoryPolicy::default`], validated.
    pub fn memory_policy_effective(&self) -> Result<MemoryPolicy, String> {
        let policy = MemoryPolicy::default();
        policy.validate()?;
        Ok(policy)
    }

    /// Sensitivity classification: [`SensitivityConfig::default`] (off).
    ///
    /// [`SensitivityConfig::default`]: crate::core::sensitivity::SensitivityConfig
    #[must_use]
    pub fn sensitivity_effective(&self) -> crate::core::sensitivity::SensitivityConfig {
        crate::core::sensitivity::SensitivityConfig::default()
    }

    /// Whether all automatic read-mode degradation is disabled. No.
    pub fn no_degrade_effective(&self) -> bool {
        false
    }

    /// Whether explicit `full`/`lines:N-M` re-reads of cached-but-changed files
    /// are served as deltas (`mode=diff`) instead of re-emitting full content.
    /// Yes.
    pub fn delta_explicit_effective(&self) -> bool {
        true
    }

    /// Fixed-context budget (tokens) (#964): the per-session footprint is
    /// checked against this in `doctor overhead` and `gain`.
    pub fn context_budget_tokens_effective(&self) -> usize {
        ContextConfig::default().budget_tokens
    }

    /// Whether later tool calls may receive matching CCR context.
    pub fn proactive_expansion_effective(&self) -> bool {
        ContextConfig::default().proactive_expansion
    }

    /// Per-response token budget for proactive CCR expansion.
    pub fn proactive_expansion_budget_tokens_effective(&self) -> usize {
        ContextConfig::default().proactive_expansion_budget_tokens
    }

    /// Normalized BM25 score threshold for proactive CCR expansion.
    pub fn proactive_expansion_threshold_effective(&self) -> f64 {
        ContextConfig::default().proactive_expansion_threshold
    }

    /// Maximum archive age considered for proactive expansion.
    pub fn proactive_expansion_max_age_secs_effective(&self) -> u64 {
        ContextConfig::default().proactive_expansion_max_age_secs
    }

    /// Archive disk ceiling (MB).
    pub fn archive_max_disk_mb_effective(&self) -> u64 {
        ArchiveConfig::default().max_disk_mb
    }

    /// Archive retention (hours).
    pub fn archive_max_age_hours_effective(&self) -> u64 {
        ArchiveConfig::default().max_age_hours
    }

    /// On-disk ceiling (MB) for the persisted BM25 index. Single source of
    /// truth for `save`/`load`, `cache prune`, and the doctor health check.
    /// Decoupled from the RAM profile so large repos persist instead of
    /// rebuilding forever (issue #249).
    pub fn bm25_max_cache_mb_effective(&self) -> u64 {
        DEFAULT_BM25_PERSIST_MB
    }
}

impl Config {
    /// Safely mutate and persist the config. Reads the file (bypassing the
    /// cache), applies `f`, then writes minimally. Refuses (returns `Err`) when
    /// the file exists but is unparseable, so a typo can never clobber a
    /// customized config (#443). Returns the saved `Config`.
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
        format!("Config: {global_path}\n\n{content}")
    }
}
