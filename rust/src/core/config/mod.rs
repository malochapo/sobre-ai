use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Cache payload for [`Config::load_arc`]: the shared config alongside the
/// content hash of the file it was built from, so a
/// later load re-reads + re-hashes and only rebuilds on a real change.
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
    ProxyConfig, ProxyProvider, Upstreams, is_local_proxy_url, normalize_url_opt, upstream_drifted,
    upstream_ids,
};

/// On-disk ceiling (MB) for the persisted BM25 index.
///
/// 512 MB compressed covers
/// essentially every real repo; RAM pressure is governed separately by the
/// eviction orchestrator (which measures real heap).
pub const DEFAULT_BM25_PERSIST_MB: u64 = 512;

/// Fixed-context budget: the per-session footprint sobre adds — tool
/// schemas + MCP instructions + auto-loaded rules files + the wakeup briefing —
/// is checked against this in `doctor overhead` and `gain`.
pub const CONTEXT_BUDGET_TOKENS: usize = 8000;

/// Graph indexes untouched for longer than this are purged.
pub const GRAPH_INDEX_MAX_AGE_HOURS: u64 = 48;

/// Cap on files the graph index scans, bounding its RAM.
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
///
/// What each key means for the user is written once, in `schema/`, which
/// `config init`, `config show` and `config-keys.md` print. The comments here
/// say only what the code needs to know.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub passthrough_commands: Vec<String>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Clamped to 0–1; [`DEFAULT_COMPRESSION_LEVEL`] when unset. Read through
    /// `CompressionLevel::effective`, which applies the session degrade.
    #[serde(
        default = "serde_defaults::default_compression_level",
        deserialize_with = "serde_defaults::deserialize_compression_level",
        skip_serializing_if = "serde_defaults::is_default_compression_level"
    )]
    pub compression_level: f64,
    /// As written: readers expand `~`/`$HOME` with `pathutil::expand_user_path`.
    #[serde(default)]
    pub search_roots: Vec<String>,
    #[serde(default)]
    pub debug_log: bool,
    /// Emitted verbatim into hook commands, which a shell expands; never into
    /// launchd/systemd units, which do not.
    #[serde(default)]
    pub hook_binary: Option<String>,
    /// `None` resolves to the 2-minute default in `shell::exec`.
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,
    #[serde(default)]
    pub ignore: Vec<String>,
    /// `None` is `minilm`; aliases resolve in `embeddings::model_registry`.
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
    /// customized config. Returns the saved `Config`.
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
                     by hand, then retry"
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
}
