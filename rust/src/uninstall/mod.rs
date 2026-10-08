mod agents;
mod binary;
mod parsers;

use std::fs;
use std::path::{Path, PathBuf};

use agents::{remove_hook_files, remove_mcp_configs, remove_skills};

pub(super) fn backup_before_modify(path: &Path, dry_run: bool) {
    if dry_run {
        return;
    }
    if path.exists() {
        let bak = bak_path_for(path);
        let _ = fs::copy(path, &bak);
    }
}

pub fn bak_path_for(path: &Path) -> PathBuf {
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{filename}.sobre.bak"))
}

fn cleanup_bak(path: &Path) {
    let bak = bak_path_for(path);
    if bak.exists() {
        let _ = fs::remove_file(&bak);
    }
}

pub(super) fn shorten(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Write `content` to `path` only if not in dry-run mode.
pub(super) fn safe_write(path: &Path, content: &str, dry_run: bool) -> Result<(), std::io::Error> {
    if dry_run {
        return Ok(());
    }
    fs::write(path, content)?;
    // If we successfully wrote the cleaned file, the backup is no longer needed.
    cleanup_bak(path);
    Ok(())
}

/// Remove `path` only if not in dry-run mode.
pub(super) fn safe_remove(path: &Path, dry_run: bool) -> Result<(), std::io::Error> {
    if dry_run {
        return Ok(());
    }
    fs::remove_file(path)?;
    // If we successfully removed the file, also remove its backup.
    cleanup_bak(path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Help
// ---------------------------------------------------------------------------

pub fn print_help() {
    println!(
        "\
sobre uninstall — remove sobre cleanly

USAGE:
    sobre uninstall [OPTIONS]

OPTIONS:
    --dry-run        Preview every change without modifying anything
    --keep-config    Preserve MCP configs and rules (for a later reinstall)
    --keep-binary    Leave the sobre binary in place
    -h, --help       Show this help and exit (does NOT uninstall)

WHAT IT REMOVES:
    • Running processes (proxy) and autostart entries
    • The proxy base URLs written into agent configs (if the proxy was enabled)
    • MCP server configs and hooks from every detected AI tool/IDE
    • Skill directories
    • The data directory and the sobre binary

    Modified files are backed up as <file>.sobre.bak before removal.

EXAMPLES:
    sobre uninstall --dry-run     # see exactly what would change
    sobre uninstall               # full clean removal"
    );
}

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

pub fn run(dry_run: bool, keep_config: bool, keep_binary: bool) {
    let Some(home) = dirs::home_dir() else {
        tracing::warn!("Could not determine home directory");
        return;
    };

    let mode_label = if keep_config {
        "uninstall --keep-config"
    } else {
        "uninstall"
    };

    if dry_run {
        println!("\n  sobre {mode_label} --dry-run\n  ──────────────────────────────────\n");
        println!("  Preview mode — no files will be modified.\n");
    } else {
        println!("\n  sobre {mode_label}\n  ──────────────────────────────────\n");
    }

    if keep_config {
        println!("  Mode: keep-config (MCP configs and rules preserved for reinstall)\n");
    }

    // Stop everything first so nothing respawns or holds the files/data we remove next.
    binary::stop_processes(dry_run);

    let mut removed_any = false;

    if dry_run {
        crate::proxy_setup::preview_proxy_cleanup(&home);
    } else {
        crate::proxy_setup::uninstall_proxy_env(&home, false);
    }

    if keep_config {
        println!("  · Skipped: MCP configs (--keep-config)");
        println!("  · Skipped: skills (--keep-config)");
    } else {
        removed_any |= remove_mcp_configs(&home, dry_run);
        removed_any |= remove_skills(&home, dry_run);
        if !dry_run {
            try_claude_mcp_remove();
        }
    }

    removed_any |= remove_hook_files(&home, dry_run);

    if dry_run {
        println!("  Would remove proxy autostart (LaunchAgent/systemd)");
    } else {
        crate::proxy_autostart::uninstall(true);
    }

    if !dry_run {
        cleanup_bak_files(&home);
    }

    removed_any |= remove_data_dir(&home, dry_run);

    // Last filesystem step: every file-removal pass above has run, so
    // installer-created directories that are empty now stay empty.
    if !dry_run {
        sweep_empty_installer_dirs(&home);
    }

    // Remove the binary itself last: once it's gone we can't re-exec, and on Unix the
    // running process keeps working until exit.
    removed_any |= binary::remove_binaries(&home, dry_run, keep_binary);

    println!();

    if removed_any {
        println!("  ──────────────────────────────────");
        if dry_run {
            println!(
                "  The above changes WOULD be applied.\n  Run `sobre {mode_label}` to execute.\n"
            );
        } else if keep_config {
            println!(
                "  Runtime data removed. MCP configs preserved for reinstall.\n  \
                 Reinstall with: cargo install sobre\n"
            );
        } else {
            println!(
                "  sobre fully removed.\n  \
                 Verify with: command -v sobre   # should print nothing\n"
            );
        }
    } else {
        println!("  Nothing to remove — sobre was not configured.\n");
    }
}

// ---------------------------------------------------------------------------
// Data directory
// ---------------------------------------------------------------------------

/// Every sobre directory an uninstall must delete, de-duplicated and
/// order-preserving: resolved through [`core::paths`](crate::core::paths) for
/// every XDG category (config/data/state/cache, honoring `SOBRE_*_DIR` and
/// `XDG_*` overrides).
fn data_dirs_to_remove() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    let push = |dirs: &mut Vec<PathBuf>, p: PathBuf| {
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    };

    // Canonical XDG categories actually written at runtime.
    for resolved in [
        crate::core::paths::config_dir(),
        crate::core::paths::data_dir(),
        crate::core::paths::state_dir(),
        crate::core::paths::cache_dir(),
    ]
    .into_iter()
    .flatten()
    {
        push(&mut dirs, resolved);
    }

    dirs
}

fn remove_data_dir(home: &Path, dry_run: bool) -> bool {
    let mut removed = false;

    let dirs_to_remove = data_dirs_to_remove();

    for data_dir in &dirs_to_remove {
        if !data_dir.exists() {
            continue;
        }
        let short = shorten(data_dir, home);
        if dry_run {
            println!("  Would remove data directory ({short})");
            removed = true;
            continue;
        }
        match fs::remove_dir_all(data_dir) {
            Ok(()) => {
                println!("  ✓ Data directory removed ({short})");
                removed = true;
            }
            Err(e) => tracing::warn!("Failed to remove {short}: {e}"),
        }
    }

    // Project-local .sobre/ in CWD
    if let Ok(cwd) = std::env::current_dir() {
        let project_dir = cwd.join(".sobre");
        if project_dir.is_dir() {
            if dry_run {
                println!("  Would remove {}", project_dir.display());
                removed = true;
            } else if fs::remove_dir_all(&project_dir).is_ok() {
                println!("  ✓ Removed {}", project_dir.display());
                removed = true;
            }
        }
    }

    if !removed {
        println!("  · No data directory found");
    }
    removed
}

fn try_claude_mcp_remove() {
    let result = std::process::Command::new("claude")
        .args(["mcp", "remove", "sobre", "--scope", "user"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match result {
        Ok(s) if s.success() => println!("  ✓ Removed sobre from Claude MCP registry"),
        _ => {} // claude CLI not available or already removed
    }
}

// ---------------------------------------------------------------------------
// .bak cleanup: remove orphaned backup files after successful surgical removal
// ---------------------------------------------------------------------------

/// Every directory the installer may have written backups or files into:
/// agent config roots and their well-known subdirectories.
fn scan_dirs(home: &Path) -> Vec<PathBuf> {
    let base_dirs: Vec<PathBuf> = vec![
        home.join(".cursor"),
        home.join(".claude"),
        crate::core::editor_registry::claude_state_dir(home),
        crate::core::editor_registry::zed_config_dir(home),
        home.join(".gemini"),
        crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex")),
        home.join(".codeium"),
        home.join(".codeium/windsurf"),
        home.join(".config/opencode"),
        home.join(".config/zed"),
        home.join(".copilot"),
    ];

    // Installers write into well-known subdirectories (Copilot's hooks/, the
    // skill dirs). read_dir below is non-recursive, so each one is scanned on
    // its own (GL #558).
    const KNOWN_SUBDIRS: [&str; 2] = ["hooks", "skills"];
    let mut dirs_to_scan: Vec<PathBuf> = Vec::with_capacity(base_dirs.len() * 3);
    for dir in base_dirs {
        for sub in KNOWN_SUBDIRS {
            let p = dir.join(sub);
            if p.is_dir() {
                dirs_to_scan.push(p);
            }
        }
        dirs_to_scan.push(dir);
    }

    dirs_to_scan
}

fn cleanup_bak_files(home: &Path) {
    let dirs_to_scan = scan_dirs(home);
    let mut cleaned = 0;
    for dir in &dirs_to_scan {
        if !dir.exists() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.contains(".sobre.invalid.") && name_str.ends_with(".bak") {
                    let _ = fs::remove_file(entry.path());
                    cleaned += 1;
                    continue;
                }
                if name_str.ends_with(".sobre.bak") {
                    let original_name = name_str.trim_end_matches(".sobre.bak");
                    let original = entry.path().with_file_name(original_name);
                    if original.exists() {
                        match fs::read_to_string(&original) {
                            Ok(c) if !c.contains("sobre") => {
                                let _ = fs::remove_file(entry.path());
                                cleaned += 1;
                            }
                            _ => {}
                        }
                    } else {
                        let _ = fs::remove_file(entry.path());
                        cleaned += 1;
                    }
                    continue;
                }
                // Plain .bak files next to known config files (created by
                // config_io). Removed whether or not the original still exists:
                // when uninstall deletes a config file that only contained
                // sobre content, its backup would otherwise be orphaned.
                if name_str.ends_with(".bak")
                    && !name_str.contains(".sobre")
                    && let Ok(bak_content) = fs::read_to_string(entry.path())
                    && bak_content.contains("sobre")
                {
                    let _ = fs::remove_file(entry.path());
                    cleaned += 1;
                }
            }
        }
    }

    if cleaned > 0 {
        println!("  ✓ Cleaned up {cleaned} backup file(s)");
    }
}

/// Sweep now-empty installer-created directories (hooks/, skills/).
/// `fs::remove_dir` refuses to delete non-empty directories, so
/// anything still holding user content survives untouched. Runs as the last
/// filesystem step of `run()` — after every file-removal pass has finished.
fn sweep_empty_installer_dirs(home: &Path) {
    let mut swept = 0;
    for dir in scan_dirs(home) {
        let is_installer_dir = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| matches!(n, "hooks" | "skills"));
        if is_installer_dir && fs::remove_dir(&dir).is_ok() {
            swept += 1;
        }
    }
    if swept > 0 {
        println!("  ✓ Removed {swept} empty installer director(y/ies)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn data_dirs_to_remove_covers_canonical_xdg_categories() {
        // `core::paths::*_dir()` reads the (test-sandboxed) data-dir env, which a
        // parallel `isolated_data_dir()` repoints under `test_env_lock`. We resolve
        // those dirs twice — once inside `data_dirs_to_remove` and once in the
        // assertion loop — so without the lock the value can flip between the two
        // reads (override active → dropped) and the set won't contain the second
        // value. Hold the lock so the env stays fixed across both reads (#991).
        let _lock = crate::core::data_dir::test_env_lock();
        let dirs = data_dirs_to_remove();

        // The set must include whatever core::paths resolves for every XDG
        // category (config, data, state, cache).
        for resolved in [
            crate::core::paths::config_dir(),
            crate::core::paths::data_dir(),
            crate::core::paths::state_dir(),
            crate::core::paths::cache_dir(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                dirs.contains(&resolved),
                "uninstall would NOT remove canonical dir: {}",
                resolved.display()
            );
        }

        // Each directory is listed exactly once (removed once, no churn).
        let mut seen = HashSet::new();
        for d in &dirs {
            assert!(seen.insert(d.clone()), "duplicate dir: {}", d.display());
        }
    }
}
