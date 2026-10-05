//! Typed XDG base-directory resolvers for lean-ctx (GH #408 / GL #602).
//!
//! One typed resolver per XDG category, so `config.toml` never mixes with
//! runtime data and the config dir can stay read-only.
//!
//! Each category resolves to its own `LEAN_CTX_<CATEGORY>_DIR` override, else
//! its XDG base: config `$XDG_CONFIG_HOME/lean-ctx`, data
//! `$XDG_DATA_HOME/lean-ctx`, state `$XDG_STATE_HOME/lean-ctx`, cache
//! `$XDG_CACHE_HOME/lean-ctx`. An override only ever moves its own category.
//!
//! Determinism (#498): every resolver is a pure function of environment + HOME;
//! no timestamps, counters or randomness.

use std::path::{Path, PathBuf};

use super::data_dir::{ensure_dir_permissions, lean_ctx_data_dir};

/// Reads a directory path from `name`, treating empty/whitespace as unset.
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Resolves an XDG base directory (e.g. `~/.config`), honoring the `env_name`
/// override and falling back to `$HOME/<home_fallback>`. Returns the base only;
/// callers append `lean-ctx`.
fn xdg_base(env_name: &str, home_fallback: &str) -> Result<PathBuf, String> {
    if let Some(p) = env_path(env_name) {
        return Ok(p);
    }
    dirs::home_dir()
        .map(|h| h.join(home_fallback))
        .ok_or_else(|| "Cannot determine home directory".to_string())
}

/// Pure resolution order for a category: explicit override > XDG default
/// (`<base>/lean-ctx`).
fn resolve(category_override: Option<PathBuf>, xdg_base_dir: &Path) -> PathBuf {
    category_override.unwrap_or_else(|| xdg_base_dir.join("lean-ctx"))
}

/// Shared resolver for the config/state/cache categories.
// Under `#[cfg(test)]` the body always succeeds (returns the sandbox); the
// fallible XDG resolution only runs in real builds.
#[cfg_attr(test, allow(clippy::unnecessary_wraps))]
fn category_dir(cat_env: &str, xdg_env: &str, home_fallback: &str) -> Result<PathBuf, String> {
    let category_override = env_path(cat_env);

    // A category override always wins, even under #[cfg(test)] — this lets the
    // RO-config sandbox integration test point each category at a temp dir.
    #[cfg(test)]
    {
        if let Some(p) = category_override {
            ensure_dir_permissions(&p);
            return Ok(p);
        }
        // Unit tests share one per-process sandbox so stray store writes can't
        // escape to a developer's real dirs. The branch logic itself is covered
        // by the pure `resolve` tests below.
        let _ = (xdg_env, home_fallback);
        Ok(super::data_dir::test_sandbox_dir())
    }
    #[cfg(not(test))]
    {
        let base = xdg_base(xdg_env, home_fallback)?;
        let dir = resolve(category_override, &base);
        ensure_dir_permissions(&dir);
        Ok(dir)
    }
}

/// Config directory — `config.toml`, shell hooks, `env.sh`. RO-safe.
/// Override: `LEAN_CTX_CONFIG_DIR`; default `$XDG_CONFIG_HOME/lean-ctx`.
pub fn config_dir() -> Result<PathBuf, String> {
    category_dir("LEAN_CTX_CONFIG_DIR", "XDG_CONFIG_HOME", ".config")
}

/// A member (a file or sub-directory) of the config dir.
pub fn config_dir_member(sub: &str) -> Result<PathBuf, String> {
    Ok(config_dir()?.join(sub))
}

/// Data directory — sessions, vectors, graphs, knowledge, archives, memory.
///
/// Delegates to [`lean_ctx_data_dir`]: `LEAN_CTX_DATA_DIR`, else
/// `$XDG_DATA_HOME/lean-ctx`.
pub fn data_dir() -> Result<PathBuf, String> {
    lean_ctx_data_dir()
}

/// State directory — events, stats, logs, journals, ledgers, captured keys.
/// Override: `LEAN_CTX_STATE_DIR`; default `$XDG_STATE_HOME/lean-ctx`.
pub fn state_dir() -> Result<PathBuf, String> {
    category_dir("LEAN_CTX_STATE_DIR", "XDG_STATE_HOME", ".local/state")
}

/// Cache directory — semantic cache, models, learned patterns. tmpfs-safe.
/// Override: `LEAN_CTX_CACHE_DIR`; default `$XDG_CACHE_HOME/lean-ctx`.
pub fn cache_dir() -> Result<PathBuf, String> {
    category_dir("LEAN_CTX_CACHE_DIR", "XDG_CACHE_HOME", ".cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_override_then_xdg() {
        let over = PathBuf::from("/over/ride");
        let base = PathBuf::from("/xdg/base");

        assert_eq!(resolve(Some(over.clone()), &base), over);
        assert_eq!(resolve(None, &base), PathBuf::from("/xdg/base/lean-ctx"));
    }

    #[test]
    fn xdg_base_honors_env_then_home_fallback() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        crate::test_env::set_var("XDG_CONFIG_HOME", tmp.path());
        let from_env = xdg_base("XDG_CONFIG_HOME", ".config").unwrap();
        crate::test_env::remove_var("XDG_CONFIG_HOME");
        assert_eq!(from_env, tmp.path());

        // Unset var → falls back to $HOME/<home_fallback>.
        let fallback = xdg_base("LEAN_CTX_NONEXISTENT_XDG_VAR", ".cache").unwrap();
        assert!(fallback.ends_with(".cache"), "got: {}", fallback.display());
    }

    #[test]
    fn config_dir_honors_explicit_override() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        crate::test_env::set_var("LEAN_CTX_CONFIG_DIR", tmp.path());
        let got = config_dir().unwrap();
        crate::test_env::remove_var("LEAN_CTX_CONFIG_DIR");
        assert_eq!(got, tmp.path());
    }

    #[test]
    fn state_and_cache_dirs_honor_explicit_overrides() {
        let _lock = crate::core::data_dir::test_env_lock();
        let state = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        crate::test_env::set_var("LEAN_CTX_STATE_DIR", state.path());
        crate::test_env::set_var("LEAN_CTX_CACHE_DIR", cache.path());
        let got_state = state_dir().unwrap();
        let got_cache = cache_dir().unwrap();
        crate::test_env::remove_var("LEAN_CTX_STATE_DIR");
        crate::test_env::remove_var("LEAN_CTX_CACHE_DIR");
        assert_eq!(got_state, state.path());
        assert_eq!(got_cache, cache.path());
    }

    #[test]
    fn data_dir_matches_lean_ctx_data_dir() {
        let _guard = crate::core::data_dir::isolated_data_dir();
        assert_eq!(
            data_dir().unwrap(),
            crate::core::data_dir::lean_ctx_data_dir().unwrap()
        );
    }
}
