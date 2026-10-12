//! `sobre uninstall`: undoes `init` and removes sobre's data and binary.

mod agents;
mod binary;

use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn shorten(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

/// Removes sobre. `options` says what to keep; any other argument stopped
/// the command line before this runs, since uninstall removes everything.
pub(crate) fn run(options: &crate::cli::args::UninstallArgs) {
    let Some(home) = dirs::home_dir() else {
        eprintln!("uninstall: cannot determine the home directory");
        return;
    };

    println!("\n  sobre uninstall\n");

    // Stop the proxy first, so it cannot rewrite what is removed next.
    binary::stop_proxy();
    crate::proxy_setup::uninstall_proxy_env(&home, false);
    crate::proxy_autostart::uninstall(true);

    let mut removed_any = agents::remove_agent_files(&home);
    removed_any |= remove_data_dirs(&home, options.keep_config);
    // The binary goes last: on Unix the running process keeps working.
    removed_any |= binary::remove_binary(&home, options.keep_binary);

    println!();
    if removed_any {
        println!("  sobre is removed. Restart your agents so they drop its server.\n");
    } else {
        println!("  Nothing to remove: sobre is not installed here.\n");
    }
}

// ---------------------------------------------------------------------------
// Data directories
// ---------------------------------------------------------------------------

/// sobre's directories, with what each holds: config, data, state and cache,
/// as resolved through [`core::paths`](crate::core::paths) (honoring the
/// `SOBRE_*_DIR` and `XDG_*` overrides).
fn data_dirs() -> Vec<(PathBuf, &'static str)> {
    let mut dirs: Vec<(PathBuf, &'static str)> = Vec::new();
    for (dir, what) in [
        (crate::core::paths::config_dir(), "config"),
        (crate::core::paths::data_dir(), "usage, indexes"),
        (crate::core::paths::state_dir(), "logs, saved outputs"),
        (crate::core::paths::cache_dir(), "downloaded models"),
    ] {
        if let Ok(dir) = dir
            && !dirs.iter().any(|(d, _)| *d == dir)
        {
            dirs.push((dir, what));
        }
    }
    dirs
}

fn remove_data_dirs(home: &Path, keep_config: bool) -> bool {
    let kept = crate::core::paths::config_dir()
        .ok()
        .filter(|_| keep_config);
    remove_dirs(home, &data_dirs(), kept.as_deref())
}

/// Deletes each of `dirs` but `kept`, and returns whether one held anything.
/// An empty directory is no install (resolving a directory can create it):
/// it is removed without a word.
fn remove_dirs(home: &Path, dirs: &[(PathBuf, &str)], kept: Option<&Path>) -> bool {
    let mut removed = false;
    for (dir, what) in dirs {
        if fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none()) {
            if fs::remove_dir(dir).is_ok() {
                remove_empty_parents(dir, home);
            }
            continue;
        }
        if !dir.exists() {
            continue;
        }
        let short = shorten(dir, home);
        if kept == Some(dir.as_path()) {
            println!("  · Kept {short} (--keep-config)");
            continue;
        }
        removed = true;
        match fs::remove_dir_all(dir) {
            Ok(()) => {
                println!("  ✓ Deleted {short} ({what})");
                remove_empty_parents(dir, home);
            }
            Err(e) => println!("  ✗ Could not delete {short}: {e}"),
        }
    }
    removed
}

/// `~/.local/state` and friends, when sobre was their only user.
fn remove_empty_parents(dir: &Path, home: &Path) {
    let mut parent = dir.parent();
    while let Some(p) = parent
        && p != home
        && p.starts_with(home)
        && fs::remove_dir(p).is_ok()
    {
        parent = p.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Directories that hold nothing are no install: they go without a word,
    /// and uninstall can say there was nothing to remove.
    #[test]
    fn empty_data_dirs_are_nothing_to_remove() {
        let home = tempfile::tempdir().unwrap();
        let dirs: Vec<(PathBuf, &str)> = [".config/sobre", ".local/share/sobre"]
            .into_iter()
            .map(|rel| (home.path().join(rel), "data"))
            .collect();
        for (dir, _) in &dirs {
            fs::create_dir_all(dir).unwrap();
        }
        assert!(!remove_dirs(home.path(), &dirs, None));
        assert!(dirs.iter().all(|(dir, _)| !dir.exists()));
        assert!(!home.path().join(".local").exists());

        fs::create_dir_all(&dirs[1].0).unwrap();
        fs::write(dirs[1].0.join("usage.jsonl"), "{}").unwrap();
        assert!(remove_dirs(home.path(), &dirs, None));
        assert!(!dirs[1].0.exists());
    }

    #[test]
    fn data_dirs_cover_every_category_once() {
        // `core::paths::*_dir()` reads the (test-sandboxed) data-dir env, which a
        // parallel `isolated_data_dir()` repoints under `test_env_lock`; hold the
        // lock so both reads below see the same values.
        let _lock = crate::core::data_dir::test_env_lock();
        let dirs: Vec<PathBuf> = data_dirs().into_iter().map(|(d, _)| d).collect();
        for resolved in [
            crate::core::paths::config_dir(),
            crate::core::paths::data_dir(),
            crate::core::paths::state_dir(),
            crate::core::paths::cache_dir(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(dirs.contains(&resolved), "missing {}", resolved.display());
        }
        let unique: HashSet<_> = dirs.iter().collect();
        assert_eq!(unique.len(), dirs.len());
    }
}
