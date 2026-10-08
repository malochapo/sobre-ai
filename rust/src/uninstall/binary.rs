//! Proxy shutdown and binary removal for `sobre uninstall`. Both steps are
//! best-effort and never abort the rest of the uninstall.

use std::fs;
use std::path::{Path, PathBuf};

use super::shorten;

/// Stops sobre's proxy: its autostart unit, then the proxy answering on the
/// configured port. Nothing else is signalled. MCP servers belong to the agents
/// that started them and end when those agents restart; hooks and CLI
/// commands are short-lived.
pub(super) fn stop_proxy(dry_run: bool) {
    let port = crate::proxy_setup::default_port();
    if dry_run {
        println!("  Would stop the sobre proxy (autostart unit, and port {port})");
        return;
    }

    crate::proxy_autostart::stop();
    if let Some(pid) = crate::proxy_autostart::stop_proxy_on_port(port) {
        println!("  ✓ Proxy stopped (PID {pid}, port {port})");
    }
}

/// How a candidate binary path should be handled.
enum Disposition {
    /// Safe to delete (a managed copy or a PATH symlink we created).
    Remove,
    /// A dev build inside a cargo `target/` dir — never touch the user's repo build.
    DevBuild,
    /// Installed by a package manager that tracks it — defer to that manager.
    Cargo,
}

fn classify(path: &Path) -> Disposition {
    let p = path.to_string_lossy();
    if p.contains("/target/release/") || p.contains("/target/debug/") {
        Disposition::DevBuild
    } else if p.contains("/.cargo/") {
        Disposition::Cargo
    } else {
        Disposition::Remove
    }
}

/// The binary the agents were connected to (`init` writes its absolute path
/// into every agent config), and the one running now.
fn candidate_paths() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from(
        crate::core::portable_binary::resolve_portable_binary(),
    )];
    if let Ok(exe) = std::env::current_exe() {
        out.push(exe);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| p.is_absolute() && seen.insert(p.clone()));
    out
}

/// Removes the sobre binary where it is safe to: never a build in a cargo
/// `target/` directory, and never one cargo manages. Returns true when a
/// binary was (or, with `dry_run`, would be) removed.
pub(super) fn remove_binary(home: &Path, dry_run: bool, keep_binary: bool) -> bool {
    if keep_binary {
        println!("  · Kept the binary (--keep-binary)");
        return false;
    }

    let mut removed = false;
    for path in candidate_paths() {
        // `symlink_metadata` does not follow symlinks: a PATH symlink is
        // removed as the link itself, never its target.
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let short = shorten(&path, home);
        match classify(&path) {
            Disposition::DevBuild => {
                println!("  · Kept {short} (a build in a cargo target directory)");
            }
            Disposition::Cargo => {
                println!(
                    "  · Kept {short}: installed with cargo, remove it with `cargo uninstall sobre`"
                );
            }
            Disposition::Remove if dry_run => {
                println!("  Would delete the binary {short}");
                removed = true;
            }
            Disposition::Remove => {
                let res = if meta.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    // Unlinking a running executable is allowed on Unix (the
                    // inode lives until the process exits).
                    fs::remove_file(&path)
                };
                match res {
                    Ok(()) => {
                        println!("  ✓ Deleted the binary {short}");
                        removed = true;
                    }
                    // Windows refuses to delete a running .exe.
                    Err(e) => println!(
                        "  ✗ Could not delete {short} ({e}); delete it after this process exits"
                    ),
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_builds_are_never_removed() {
        assert!(matches!(
            classify(Path::new("/home/u/sobre/rust/target/release/sobre")),
            Disposition::DevBuild
        ));
        assert!(matches!(
            classify(Path::new("/home/u/proj/target/debug/sobre")),
            Disposition::DevBuild
        ));
    }

    #[test]
    fn package_managers_are_deferred() {
        assert!(matches!(
            classify(Path::new("/home/u/.cargo/bin/sobre")),
            Disposition::Cargo
        ));
    }

    #[test]
    fn managed_install_dirs_are_removable() {
        assert!(matches!(
            classify(Path::new("/home/u/.local/bin/sobre")),
            Disposition::Remove
        ));
        assert!(matches!(
            classify(Path::new("/usr/local/bin/sobre")),
            Disposition::Remove
        ));
    }

    #[test]
    fn keep_binary_skips_removal() {
        let home = std::env::temp_dir();
        assert!(!remove_binary(&home, true, true));
    }
}
