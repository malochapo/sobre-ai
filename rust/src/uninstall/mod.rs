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
        (
            crate::core::paths::data_dir(),
            "usage, sessions, knowledge, indexes",
        ),
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
    let config_dir = crate::core::paths::config_dir().ok();
    let mut removed = false;
    for (dir, what) in data_dirs() {
        if !dir.exists() {
            continue;
        }
        let short = shorten(&dir, home);
        if keep_config && config_dir.as_ref() == Some(&dir) {
            println!("  · Kept {short} (--keep-config)");
            continue;
        }
        removed = true;
        match fs::remove_dir_all(&dir) {
            Ok(()) => {
                println!("  ✓ Deleted {short} ({what})");
                // `~/.local/state` and friends, when sobre was their only user.
                let mut parent = dir.parent();
                while let Some(p) = parent
                    && p != home
                    && p.starts_with(home)
                    && fs::remove_dir(p).is_ok()
                {
                    parent = p.parent();
                }
            }
            Err(e) => println!("  ✗ Could not delete {short}: {e}"),
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
