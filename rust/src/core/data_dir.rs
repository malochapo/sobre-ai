use std::path::PathBuf;

/// Resolve the sobre data directory.
///
/// `SOBRE_DATA_DIR` when set, else `$XDG_DATA_HOME/sobre` (default
/// `~/.local/share/sobre`), so the config dir holds only config and stays
/// RO-sandbox-safe.
pub fn sobre_data_dir() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("SOBRE_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            ensure_dir_permissions(&p);
            return Ok(p);
        }
    }

    // Test sandbox: without this, any unit test that triggers a
    // store write (usage ledger, context ledger, sessions, ...)
    // silently pollutes the developer's real data dir. Tests that
    // set SOBRE_DATA_DIR keep full control (handled above); everyone else
    // lands in a per-process temp dir and physically cannot touch real data.
    #[cfg(test)]
    {
        Ok(test_sandbox_dir())
    }

    #[cfg(not(test))]
    {
        resolve_home_data_dir()
    }
}

/// Per-process temp sandbox used as the default data dir under `#[cfg(test)]`,
/// so any store write from a test fixture lands in a throwaway dir instead of
/// the developer's real data dir. Tests that need an *empty*,
/// private dir should use [`isolated_data_dir`] instead (the shared sandbox is
/// not empty: parallel tests write into it).
#[cfg(test)]
pub(crate) fn test_sandbox_dir() -> PathBuf {
    static TEST_SANDBOX: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    TEST_SANDBOX
        .get_or_init(|| {
            let d = std::env::temp_dir().join(format!("sobre-testdata-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&d);
            d
        })
        .clone()
}

/// Home-based resolution: `$XDG_DATA_HOME/sobre`. Split out so it stays
/// unit-testable despite the test sandbox above.
#[cfg_attr(test, allow(dead_code))]
fn resolve_home_data_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    let xdg_data = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| home.join(".local").join("share"), PathBuf::from);
    let data_dir = xdg_data.join("sobre");
    ensure_dir_permissions(&data_dir);
    Ok(data_dir)
}

#[cfg(unix)]
pub(crate) fn ensure_dir_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if path.is_dir() {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
}

#[cfg(not(unix))]
pub(crate) fn ensure_dir_permissions(_path: &std::path::Path) {}

pub fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let mutex = LOCK.get_or_init(|| Mutex::new(()));
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// RAII data-dir isolation for tests: holds `test_env_lock` for
/// the guard's lifetime, points `SOBRE_DATA_DIR` at a fresh temp dir and
/// restores the env on drop — even on panic, so a failing test cannot leak
/// the override into others. Use this instead of hand-rolled
/// `set_var`/`remove_var` pairs whenever a test needs an empty, private
/// data dir (the shared per-process sandbox is NOT empty: parallel tests
/// write stores like feedback, bandit and sessions into it).
#[cfg(test)]
pub struct IsolatedDataDir {
    tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl IsolatedDataDir {
    pub fn path(&self) -> &std::path::Path {
        self.tmp.path()
    }
}

/// Category env vars pointed at the isolated temp dir so all four XDG
/// categories (config/data/state/cache) collapse onto it in tests.
#[cfg(test)]
const ISOLATED_ENV_VARS: &[&str] = &[
    "SOBRE_DATA_DIR",
    "SOBRE_CONFIG_DIR",
    "SOBRE_STATE_DIR",
    "SOBRE_CACHE_DIR",
];

#[cfg(test)]
impl Drop for IsolatedDataDir {
    fn drop(&mut self) {
        // Struct Drop runs before field drops, so the env is restored while
        // the lock is still held.
        for var in ISOLATED_ENV_VARS {
            crate::test_env::remove_var(var);
        }
    }
}

#[cfg(test)]
pub fn isolated_data_dir() -> IsolatedDataDir {
    let guard = test_env_lock();
    let tmp = tempfile::tempdir().expect("tempdir for isolated data dir");
    for var in ISOLATED_ENV_VARS {
        crate::test_env::set_var(var, tmp.path());
    }
    IsolatedDataDir { tmp, _guard: guard }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_defaults_to_xdg_data_home() {
        // DATA resolves to $XDG_DATA_HOME/sobre (not the config dir).
        let _lock = test_env_lock();
        let xdg_config = tempfile::tempdir().unwrap();
        let xdg_data = tempfile::tempdir().unwrap();
        crate::test_env::set_var("SOBRE_DATA_DIR", "");
        crate::test_env::set_var("XDG_CONFIG_HOME", xdg_config.path());
        crate::test_env::set_var("XDG_DATA_HOME", xdg_data.path());

        let result = resolve_home_data_dir().unwrap();

        crate::test_env::remove_var("SOBRE_DATA_DIR");
        crate::test_env::remove_var("XDG_CONFIG_HOME");
        crate::test_env::remove_var("XDG_DATA_HOME");

        assert_eq!(result, xdg_data.path().join("sobre"));
    }

    #[test]
    fn sobre_data_dir_env_override() {
        let _lock = test_env_lock();
        let dir = std::env::temp_dir().join("test_data_dir_env");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        crate::test_env::set_var("SOBRE_DATA_DIR", dir.to_str().unwrap());
        let result = sobre_data_dir().unwrap();
        assert_eq!(result, dir);
        crate::test_env::remove_var("SOBRE_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
