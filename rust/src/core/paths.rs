//! Typed XDG base-directory resolvers for lean-ctx (GH #408 / GL #602).
//!
//! One typed resolver per XDG category, so `config.toml` never mixes with
//! runtime data and the config dir can stay read-only.
//!
//! ## Single-dir mode
//!
//! A non-standard `LEAN_CTX_DATA_DIR` collapses every category onto that one
//! directory (a deliberate choice, and how tests isolate themselves). Otherwise
//! each category resolves to its XDG base: config `$XDG_CONFIG_HOME/lean-ctx`,
//! data `$XDG_DATA_HOME/lean-ctx`, state `$XDG_STATE_HOME/lean-ctx`, cache
//! `$XDG_CACHE_HOME/lean-ctx`.
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

/// Pure resolution order for a category: explicit override > single-dir
/// > XDG split default (`<base>/lean-ctx`).
fn resolve(
    category_override: Option<PathBuf>,
    single: Option<PathBuf>,
    xdg_base_dir: &Path,
) -> PathBuf {
    category_override
        .or(single)
        .unwrap_or_else(|| xdg_base_dir.join("lean-ctx"))
}

/// The single directory every category collapses onto, or `None` for the
/// XDG split.
///
/// `Some` only when `LEAN_CTX_DATA_DIR` points at a non-standard location. A
/// `LEAN_CTX_DATA_DIR` equal to the standard XDG data dir
/// (`$XDG_DATA_HOME/lean-ctx`) is a DATA pin only: editors bake that value into
/// the MCP server's `env`, and collapsing on it would make the MCP process read
/// a different `config.toml` than the terminal CLI (#594).
pub(crate) fn single_dir_override() -> Option<PathBuf> {
    env_path("LEAN_CTX_DATA_DIR").filter(|p| !is_standard_xdg_data_dir(p))
}

/// True when `p` is exactly the standard XDG data dir (`$XDG_DATA_HOME/lean-ctx`,
/// default `~/.local/share/lean-ctx`). Pure function of env + HOME (no filesystem
/// access) so category resolution stays deterministic (#498).
fn is_standard_xdg_data_dir(p: &Path) -> bool {
    xdg_base("XDG_DATA_HOME", ".local/share")
        .map(|base| base.join("lean-ctx"))
        .is_ok_and(|standard| standard.as_path() == p)
}

/// Whether an editor-baked `LEAN_CTX_DATA_DIR` value (`pin`) would make that
/// editor's MCP server resolve a *different* `config.toml` than the terminal
/// CLI. Only a **non-standard** pin collapses config/state/cache onto the data
/// dir (#594); a pin equal to the standard `$XDG_DATA_HOME/lean-ctx` is a
/// data-only pin and keeps config parity. Pure function of `pin` + the current
/// process' env/HOME (the terminal's view), so `doctor` can compare an editor's
/// baked pin against the CLI's own resolution without mutating env or spawning.
pub(crate) fn data_pin_diverges_config(pin: &Path) -> bool {
    !is_standard_xdg_data_dir(pin)
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
        let dir = resolve(category_override, single_dir_override(), &base);
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
    fn resolve_prefers_override_then_single_then_xdg() {
        let over = PathBuf::from("/over/ride");
        let single = PathBuf::from("/single/dir");
        let base = PathBuf::from("/xdg/base");

        assert_eq!(
            resolve(Some(over.clone()), Some(single.clone()), &base),
            over
        );
        assert_eq!(resolve(None, Some(single.clone()), &base), single);
        assert_eq!(
            resolve(None, None, &base),
            PathBuf::from("/xdg/base/lean-ctx")
        );
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
    fn single_dir_override_honors_data_dir_env() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        crate::test_env::set_var("LEAN_CTX_DATA_DIR", tmp.path());
        let got = single_dir_override();
        crate::test_env::remove_var("LEAN_CTX_DATA_DIR");
        // A custom (non-standard) data dir is a deliberate single-dir choice.
        assert_eq!(got, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn is_standard_xdg_data_dir_matches_xdg_data_home() {
        let _lock = crate::core::data_dir::test_env_lock();
        let data_home = tempfile::tempdir().unwrap();
        crate::test_env::set_var("XDG_DATA_HOME", data_home.path());
        let is_std = is_standard_xdg_data_dir(&data_home.path().join("lean-ctx"));
        let is_custom = is_standard_xdg_data_dir(Path::new("/some/custom/lean-ctx"));
        crate::test_env::remove_var("XDG_DATA_HOME");
        assert!(is_std, "$XDG_DATA_HOME/lean-ctx is the standard data dir");
        assert!(!is_custom, "a custom path is not the standard data dir");
    }

    #[test]
    fn standard_data_pin_does_not_collapse_categories() {
        // #594: an editor (MCP env) that pins LEAN_CTX_DATA_DIR to the *standard*
        // XDG data dir must NOT drag config/state/cache along — single_dir_override
        // must return None so they keep their own XDG bases, matching the CLI.
        let _lock = crate::core::data_dir::test_env_lock();
        let home = tempfile::tempdir().unwrap();
        let xdg_config = tempfile::tempdir().unwrap();
        let xdg_data = tempfile::tempdir().unwrap();
        let data_pin = xdg_data.path().join("lean-ctx");
        crate::test_env::set_var("HOME", home.path());
        crate::test_env::set_var("XDG_CONFIG_HOME", xdg_config.path());
        crate::test_env::set_var("XDG_DATA_HOME", xdg_data.path());
        crate::test_env::set_var("LEAN_CTX_DATA_DIR", &data_pin);

        let got = single_dir_override();

        crate::test_env::remove_var("LEAN_CTX_DATA_DIR");
        crate::test_env::remove_var("XDG_DATA_HOME");
        crate::test_env::remove_var("XDG_CONFIG_HOME");
        crate::test_env::remove_var("HOME");

        assert_eq!(got, None);
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
