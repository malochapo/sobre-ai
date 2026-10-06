//! Config path resolution and disk loading.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{Config, ConfigCacheSlot};

/// Holds the most recent global `config.toml` parse error, if the file currently
/// fails to parse. When that happens `Config::load()` silently falls back to the
/// built-in defaults and only logs to stderr — which is invisible over an MCP/stdio
/// transport. Recording it here lets callers (e.g. the shell-allowlist diagnostic
/// and `lean-ctx doctor`) surface "you're on defaults because your config is broken".
static LAST_PARSE_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Returns the most recent global config parse error, or `None` if the current
/// `config.toml` parsed successfully (or no config file exists).
#[must_use]
pub fn last_config_parse_error() -> Option<String> {
    LAST_PARSE_ERROR.lock().ok().and_then(|g| g.clone())
}

fn record_parse_error(err: Option<String>) {
    if let Ok(mut guard) = LAST_PARSE_ERROR.lock() {
        *guard = err;
    }
}

impl Config {
    /// Returns the path to the global config file (`$XDG_CONFIG_HOME/lean-ctx/config.toml`).
    ///
    /// Resolves via [`crate::core::paths::config_dir`] so config lives in the
    /// RO-safe config category (GH #408 / GL #602).
    pub fn path() -> Option<PathBuf> {
        crate::core::paths::config_dir()
            .ok()
            .map(|d| d.join("config.toml"))
    }

    /// `Some(path)` when the global config the runtime *resolves* does not exist,
    /// so lean-ctx is silently on built-in defaults. `None` when a config file is
    /// present (or HOME is unresolvable).
    ///
    /// The directory is layout-dependent (XDG `~/.config/lean-ctx` vs
    /// `$LEAN_CTX_CONFIG_DIR` vs `$LEAN_CTX_DATA_DIR`) and an MCP client may launch the
    /// server in a sandbox/container with a different `$HOME`. An edit made to a
    /// *different* `config.toml` than this one is silently ignored; the block
    /// messages use this to say so out loud over MCP, where the stderr path is
    /// invisible (#540).
    #[must_use]
    pub fn missing_config_path() -> Option<PathBuf> {
        match Self::path() {
            Some(p) if !p.exists() => Some(p),
            _ => None,
        }
    }

    /// Resolves the active project root (git toplevel → cwd), cached for the
    /// process.
    pub(crate) fn find_project_root() -> Option<String> {
        static ROOT_CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        ROOT_CACHE
            .get_or_init(Self::find_project_root_inner)
            .clone()
    }

    fn find_project_root_inner() -> Option<String> {
        let cwd = std::env::current_dir().ok();

        if let Some(ref cwd) = cwd {
            // A launchd-standalone process must not shell out to `git` (which
            // stats the working tree) or adopt cwd as the project root when cwd
            // is under a TCC-protected dir (#356).
            let may_probe_cwd = crate::core::pathutil::may_probe_path(cwd);
            let git_root = if may_probe_cwd {
                std::process::Command::new("git")
                    .args(["rev-parse", "--show-toplevel"])
                    .current_dir(cwd)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            String::from_utf8(o.stdout)
                                .ok()
                                .map(|s| s.trim().to_string())
                        } else {
                            None
                        }
                    })
            } else {
                None
            };
            if let Some(root) = git_root {
                return Some(root);
            }
            if may_probe_cwd && !crate::core::pathutil::is_broad_or_unsafe_root(cwd) {
                return Some(cwd.to_string_lossy().to_string());
            }
        }
        None
    }

    /// Loads the config file, cached.
    ///
    /// The cache is keyed on a **content hash** of the file, not its mtime.
    /// mtime-only invalidation silently served a stale `Config` whenever a
    /// content edit preserved the mtime (coarse filesystem mtime resolution,
    /// `cp -p`, atomic save-then-rename, two edits within the same second). A
    /// long-lived MCP server then kept the old value while a fresh `lean-ctx
    /// doctor` process saw the new one (#406). The file is tiny, so reading +
    /// hashing it on every load is negligible and guarantees liveness.
    pub fn load() -> Self {
        (*Self::load_arc()).clone()
    }

    /// Shared-ownership variant of [`load`](Self::load): returns the cached
    /// `Arc<Config>` so the per-dispatch hot path bumps a refcount instead of
    /// deep-cloning the whole struct.
    pub fn load_arc() -> Arc<Self> {
        static CACHE: Mutex<ConfigCacheSlot> = Mutex::new(None);

        let Some(path) = Self::path() else {
            return Arc::new(Self::default());
        };

        let content = std::fs::read_to_string(&path).ok();
        let hash = content.as_deref().map(crate::core::hasher::hash_str);

        if let Ok(guard) = CACHE.lock()
            && let Some((ref cfg, ref cached)) = *guard
            && *cached == hash
        {
            return Arc::clone(cfg);
        }

        let cfg: Config = if let Some(ref content) = content {
            match toml::from_str(content) {
                Ok(c) => {
                    record_parse_error(None);
                    c
                }
                Err(e) => {
                    record_parse_error(Some(format!("{e}")));
                    tracing::warn!("config parse error in {}: {e}", path.display());
                    eprintln!(
                        "\x1b[33m[lean-ctx] WARNING: config parse error in {}: {e}\n  \
                         Using defaults. Run `lean-ctx doctor --fix` to repair.\x1b[0m",
                        path.display()
                    );
                    Self::default()
                }
            }
        } else {
            record_parse_error(None);
            Self::default()
        };

        let cfg = Arc::new(cfg);
        if let Ok(mut guard) = CACHE.lock() {
            *guard = Some((Arc::clone(&cfg), hash));
        }

        cfg
    }

    /// Loads the config file bypassing the in-memory cache. Persist paths use
    /// this (or [`Config::update_global`]) so they start from what is on disk.
    pub fn load_global() -> Self {
        Self::path().map_or_else(Self::default, |p| Self::load_global_from(&p))
    }

    /// Path-parameterized core of [`Config::load_global`] (unit-testable without
    /// the real config dir). Missing, empty, or unparseable files yield
    /// defaults; persisting callers that must not clobber a corrupt file use
    /// [`Config::update_global`], which refuses instead.
    pub(super) fn load_global_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) if !raw.trim().is_empty() => toml::from_str(&raw).unwrap_or_default(),
            _ => Self::default(),
        }
    }
}
