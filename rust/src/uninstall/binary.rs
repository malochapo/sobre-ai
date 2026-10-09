//! Proxy shutdown and binary removal for `sobre uninstall`. Both steps are
//! best-effort and never abort the rest of the uninstall.

use std::fs;
use std::path::{Path, PathBuf};

use super::shorten;

/// Stops sobre's proxy: its autostart unit, then the proxy answering on the
/// configured port. Nothing else is signalled. MCP servers belong to the agents
/// that started them and end when those agents restart; hooks and CLI
/// commands are short-lived.
pub(super) fn stop_proxy() {
    crate::proxy_autostart::stop_everywhere(false);
}

/// How a candidate binary path should be handled.
enum Disposition {
    /// Safe to delete: a copy an install script put in place (or a link to
    /// one).
    Remove,
    /// A dev build inside a cargo `target/` dir — never touch the user's repo build.
    DevBuild,
    /// Installed by `cargo install` into `~/.cargo`: cargo removes it.
    Cargo,
}

/// Decided on the path's components, whichever separator it uses, so a
/// Windows path (`C:\Users\me\.cargo\bin\sobre.exe`) is read like a Unix
/// one.
fn classify(path: &Path) -> Disposition {
    let text = path.to_string_lossy().replace('\\', "/");
    let components: Vec<&str> = text.split('/').collect();
    if components
        .windows(2)
        .any(|pair| pair[0] == "target" && matches!(pair[1], "debug" | "release"))
    {
        Disposition::DevBuild
    } else if components.contains(&".cargo") {
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
/// binary was removed.
pub(super) fn remove_binary(home: &Path, keep_binary: bool) -> bool {
    if keep_binary {
        println!("  · Kept the binary (--keep-binary)");
        return false;
    }

    let mut removed = false;
    for path in candidate_paths() {
        // `symlink_metadata` does not follow symlinks: a PATH symlink is
        // removed as the link itself, never its target.
        if fs::symlink_metadata(&path).is_err() {
            continue;
        }
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
            Disposition::Remove => {
                // Unlinking a running executable is allowed on Unix (the
                // inode lives until the process exits).
                match fs::remove_file(&path) {
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
        for path in [
            "/home/u/sobre/rust/target/release/sobre",
            "/home/u/proj/target/debug/sobre",
            r"C:\Users\u\sobre\rust\target\release\sobre.exe",
            r"D:\proj\target\debug\sobre.exe",
        ] {
            assert!(
                matches!(classify(Path::new(path)), Disposition::DevBuild),
                "{path}"
            );
        }
    }

    #[test]
    fn cargo_installs_are_left_to_cargo() {
        for path in [
            "/home/u/.cargo/bin/sobre",
            r"C:\Users\u\.cargo\bin\sobre.exe",
        ] {
            assert!(
                matches!(classify(Path::new(path)), Disposition::Cargo),
                "{path}"
            );
        }
    }

    #[test]
    fn installed_copies_are_removable() {
        for path in [
            "/home/u/.local/bin/sobre",
            "/usr/local/bin/sobre",
            r"C:\Users\u\AppData\Local\Programs\sobre\sobre.exe",
            // A directory merely named like a build or cargo dir is no match.
            "/home/u/targets/debug/sobre",
            "/home/u/my.cargo/bin/sobre",
        ] {
            assert!(
                matches!(classify(Path::new(path)), Disposition::Remove),
                "{path}"
            );
        }
    }

    #[test]
    fn keep_binary_skips_removal() {
        let home = std::env::temp_dir();
        assert!(!remove_binary(&home, true));
    }
}
