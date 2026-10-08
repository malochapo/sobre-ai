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
mod render;
pub mod risk;
pub mod schema;
mod serde_defaults;
pub mod setter;
pub use render::render_annotated_config;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_parsing;

pub use enums::{CompressionLevel, SessionDegrade};
pub use loader::last_config_parse_error;
pub use proxy::{
    ProxyConfig, ProxyProvider, Upstreams, is_local_proxy_url, normalize_url, normalize_url_opt,
    upstream_drifted, upstream_ids,
};

/// On-disk ceiling (MB) for the persisted BM25 index.
///
/// 512 MB compressed covers
/// essentially every real repo; RAM pressure is governed separately by the
/// eviction orchestrator (which measures real heap).
pub const DEFAULT_BM25_PERSIST_MB: u64 = 512;

/// Fixed-context budget (#964): the per-session footprint sobre adds — tool
/// schemas + MCP instructions + auto-loaded rules files + the wakeup briefing —
/// is checked against this in `doctor overhead` and `gain`.
pub const CONTEXT_BUDGET_TOKENS: usize = 8000;

/// Proactive CCR expansion: maximum archive content injected per tool response.
pub const PROACTIVE_EXPANSION_BUDGET_TOKENS: usize = 2000;
/// Proactive CCR expansion: minimum normalized BM25 score for an injection.
pub const PROACTIVE_EXPANSION_THRESHOLD: f64 = 0.6;
/// Proactive CCR expansion: archived content older than this is ignored.
pub const PROACTIVE_EXPANSION_MAX_AGE_SECS: u64 = 3600;

/// Graph indexes untouched for longer than this are purged.
pub const GRAPH_INDEX_MAX_AGE_HOURS: u64 = 48;

/// Compact output formats preserved verbatim instead of recompressed (#342).
pub const PRESERVE_COMPACT_FORMATS: &[&str] = &["toon"];

/// Cap on files the graph index scans, bounding its RAM (#790).
pub const GRAPH_INDEX_MAX_FILES: u64 = 15_000;

/// Share of system RAM the memory guard allows sobre to use.
pub const MAX_RAM_PERCENT: u8 = 5;

/// Idle time after which the session cache is cleared.
pub const IDLE_CACHE_TTL_SECS: u64 = 3600;

/// `compression_level` when the file does not set it: the middle of the
/// `lite` band.
pub const DEFAULT_COMPRESSION_LEVEL: f64 = 0.3;

/// Whether `v` is the default `compression_level` (so `config show` can say
/// the value is a default, not something the file sets).
#[must_use]
pub fn is_default_compression_level_value(v: f64) -> bool {
    (v - DEFAULT_COMPRESSION_LEVEL).abs() < f64::EPSILON
}

/// sobre configuration, loaded from the single `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub passthrough_commands: Vec<String>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Compression intensity, `0.0` (none) to `1.0` (densest), clamped;
    /// [`DEFAULT_COMPRESSION_LEVEL`] when the file does not set it. Picks the
    /// prose, shell-output and CRP style by band (`CompressionLevel::from_value`).
    /// File reads choose their mode per file; a call can ask for denser reads
    /// with its own `aggressiveness`.
    #[serde(
        default = "serde_defaults::default_compression_level",
        deserialize_with = "serde_defaults::deserialize_compression_level",
        skip_serializing_if = "serde_defaults::is_default_compression_level"
    )]
    pub compression_level: f64,
    /// Extra directories searched alongside the project root, as the other
    /// roots of a multi-root workspace (absolute, `~`/`$HOME` expanded).
    #[serde(default)]
    pub search_roots: Vec<String>,
    /// Opt-in (#520): write a human-readable debug log of intercepted MCP tool
    /// calls and hook routing decisions (sobre vs native, with reasons) to
    /// `<state_dir>/logs/debug.log`.
    #[serde(default)]
    pub debug_log: bool,
    /// Verbatim binary path/expression for generated agent-hook commands
    /// (#708). Users who sync agent settings (`~/.claude/settings.json`, …)
    /// across machines with different usernames need an env-based form like
    /// `$HOME/.local/bin/sobre` — agent hosts run hook commands through a
    /// shell, which expands it. When set, every hook writer emits the value verbatim instead of
    /// the machine-absolute exe path, so `init`/`doctor --fix` stop
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
            compression_level: DEFAULT_COMPRESSION_LEVEL,
            search_roots: Vec::new(),
            debug_log: false,
            hook_binary: None,
            shell_timeout_secs: None,
            ignore: Vec::new(),
            embedding_model: None,
        }
    }
}

impl Config {
    /// Safely mutate and persist the config. Reads the file (bypassing the
    /// cache), applies `f`, then writes minimally. Refuses (returns `Err`) when
    /// the file exists but is unparseable, so a typo can never clobber a
    /// customized config (#443). Returns the saved `Config`.
    pub fn update_global<F>(f: F) -> std::result::Result<Self, super::error::SobreError>
    where
        F: FnOnce(&mut Self),
    {
        let path = Self::path().ok_or_else(|| {
            super::error::SobreError::Config("cannot determine home directory".into())
        })?;
        Self::update_global_at(&path, f)
    }

    /// Path-parameterized core of [`Config::update_global`] (unit-testable).
    fn update_global_at<F>(path: &Path, f: F) -> std::result::Result<Self, super::error::SobreError>
    where
        F: FnOnce(&mut Self),
    {
        let mut cfg = match std::fs::read_to_string(path) {
            Ok(raw) if !raw.trim().is_empty() => toml::from_str::<Self>(&raw).map_err(|e| {
                super::error::SobreError::Config(
                    format!(
                        "refusing to modify an unparseable config.toml ({e}); fix it \
                     manually or run `sobre doctor --fix`, then retry"
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
    pub fn save(&self) -> std::result::Result<(), super::error::SobreError> {
        let path = Self::path().ok_or_else(|| {
            super::error::SobreError::Config("cannot determine home directory".into())
        })?;
        self.save_to(&path)
    }

    /// Path-parameterized core of [`Config::save`] (unit-testable).
    fn save_to(&self, path: &Path) -> std::result::Result<(), super::error::SobreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| super::error::SobreError::Config(e.to_string().into()))?;
        // Baseline = what loading an empty config yields. This honors serde's
        // field-level `#[serde(default)]` (which can diverge from the struct's
        // `Default` impl), so minimal mode skips exactly the keys that a fresh
        // load would produce — no spurious lines on save.
        let baseline = toml::from_str::<Self>("").unwrap_or_else(|_| Self::default());
        let defaults = toml::to_string_pretty(&baseline)
            .map_err(|e| super::error::SobreError::Config(e.to_string().into()))?;
        crate::config_io::write_toml_preserving_minimal(path, &content, &defaults)
            .map_err(|e| super::error::SobreError::Config(e.into()))?;
        Ok(())
    }

    /// Formats the current config as a human-readable string with file paths.
    pub fn show(&self) -> String {
        let global_path = Self::path().map_or_else(
            || "(config dir unresolved)".to_string(),
            |p| p.to_string_lossy().to_string(),
        );
        let content = toml::to_string_pretty(self).unwrap_or_default();
        format!("Config: {global_path}\n\n{content}")
    }
}
