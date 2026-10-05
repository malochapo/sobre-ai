use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Cache payload for [`Config::load_arc`]: the shared config alongside the
/// content hash of the file it was built from, so a
/// later load re-reads + re-hashes and only rebuilds on a real change (#406).
type ConfigCacheSlot = Option<(Arc<Config>, Option<String>)>;

mod enums;
mod loader;
mod proxy;
mod read_dedup;
pub(crate) mod read_redirect;
mod render;
pub mod risk;
pub mod schema;
mod serde_defaults;
pub mod setter;
pub mod shell_activation;
pub use render::render_annotated_config;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_parsing;

pub use enums::{CompressionLevel, RecoveryHints, ResponseVerbosity, SessionDegrade, TeeMode};
pub use loader::last_config_parse_error;
pub use proxy::{
    ProxyConfig, ProxyProvider, ResolvedProvider, RoutingRules, Upstreams, WireShape,
    is_local_proxy_url, normalize_url, normalize_url_opt, parse_route_target, upstream_drifted,
    upstream_ids,
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
/// 512 MB compressed covers
/// essentially every real repo; RAM pressure is governed separately by the
/// eviction orchestrator (which measures real heap).
pub const DEFAULT_BM25_PERSIST_MB: u64 = 512;

/// Fixed-context budget (#964): the per-session footprint lean-ctx adds — tool
/// schemas + MCP instructions + auto-loaded rules files + the wakeup briefing —
/// is checked against this in `doctor overhead` and `gain`.
pub const CONTEXT_BUDGET_TOKENS: usize = 8000;

/// Proactive CCR expansion: maximum archive content injected per tool response.
pub const PROACTIVE_EXPANSION_BUDGET_TOKENS: usize = 2000;
/// Proactive CCR expansion: minimum normalized BM25 score for an injection.
pub const PROACTIVE_EXPANSION_THRESHOLD: f64 = 0.6;
/// Proactive CCR expansion: archived content older than this is ignored.
pub const PROACTIVE_EXPANSION_MAX_AGE_SECS: u64 = 3600;

/// Disk ceiling of the zero-loss compression archive (large tool outputs).
pub const ARCHIVE_MAX_DISK_MB: u64 = 500;
/// Retention of the compression archive.
pub const ARCHIVE_MAX_AGE_HOURS: u64 = 48;

/// Compact output formats preserved verbatim instead of recompressed (#342).
pub const PRESERVE_COMPACT_FORMATS: &[&str] = &["toon"];

/// Cap on files the graph index scans, bounding its RAM (#790).
pub const GRAPH_INDEX_MAX_FILES: u64 = 15_000;

/// Share of system RAM the memory guard allows lean-ctx to use.
pub const MAX_RAM_PERCENT: u8 = 5;

/// Idle time after which the session cache is cleared.
pub const IDLE_CACHE_TTL_SECS: u64 = 3600;

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
    /// Embedding model for semantic search (`dense` builds): `minilm`
    /// (default), `nomic`, or any HuggingFace repo shipping an ONNX export
    /// and a `tokenizer.json`, as `org/repo[@revision]`. Pooling and
    /// query/document prompts come from the repo's sentence-transformers
    /// metadata. Changing it re-indexes on the next semantic search.
    #[serde(default)]
    pub embedding_model: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            passthrough_commands: Vec::new(),
            proxy: ProxyConfig::default(),
            compression_level: None,
            allow_paths: Vec::new(),
            read_only_roots: Vec::new(),
            debug_log: false,
            hook_binary: None,
            shell_timeout_secs: None,
            shell_hook: true,
            ignore: Vec::new(),
            embedding_model: None,
        }
    }
}

impl Config {
    /// Returns `true` if shell hook injection is disabled.
    pub fn shell_hook_disabled_effective(&self) -> bool {
        !self.shell_hook
    }

    /// Whether `allow_paths` opens the path jail entirely (`"/"`).
    #[must_use]
    pub fn jail_open(&self) -> bool {
        self.allow_paths.iter().any(|p| p.trim() == "/")
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
