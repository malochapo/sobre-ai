mod agents;
mod binary;
mod parsers;

use std::fs;
use std::path::{Path, PathBuf};

use agents::{
    remove_hook_files, remove_mcp_configs, remove_plan_mode_settings, remove_project_agent_files,
    remove_shell_hook, remove_skills,
};

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
    path.with_file_name(format!("{filename}.lean-ctx.bak"))
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

/// Print usage for `lean-ctx uninstall`.
///
/// This MUST stay side-effect free: `lean-ctx uninstall --help` previously fell
/// through to [`run`] and removed everything, so help is now short-circuited in
/// the CLI dispatch before any removal happens.
/// Remove a lean-ctx shell hook an older install left in the user's shell
/// files (used by `doctor --fix`).
pub fn remove_legacy_shell_hook(home: &Path) {
    agents::remove_shell_hook(home, false);
}

pub fn print_help() {
    println!(
        "\
lean-ctx uninstall — remove lean-ctx cleanly

USAGE:
    lean-ctx uninstall [OPTIONS]

OPTIONS:
    --dry-run        Preview every change without modifying anything
    --keep-config    Preserve MCP configs and rules (for a later reinstall)
    --keep-binary    Leave the lean-ctx binary in place
    -h, --help       Show this help and exit (does NOT uninstall)

WHAT IT REMOVES:
    • Running processes (proxy) and autostart entries
    • Shell hooks and proxy environment from your shell rc files
    • MCP server configs and rules from every detected AI tool/IDE
    • Skill directories and project integration files
    • The data directory and the lean-ctx binary

    Modified files are backed up as <file>.lean-ctx.bak before removal.

EXAMPLES:
    lean-ctx uninstall --dry-run     # see exactly what would change
    lean-ctx uninstall               # full clean removal"
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
        println!("\n  lean-ctx {mode_label} --dry-run\n  ──────────────────────────────────\n");
        println!("  Preview mode — no files will be modified.\n");
    } else {
        println!("\n  lean-ctx {mode_label}\n  ──────────────────────────────────\n");
    }

    if keep_config {
        println!("  Mode: keep-config (MCP configs and rules preserved for reinstall)\n");
    }

    // Stop everything first so nothing respawns or holds the files/data we remove next.
    binary::stop_processes(dry_run);

    let mut removed_any = false;

    removed_any |= remove_shell_hook(&home, dry_run);
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
    removed_any |= remove_plan_mode_settings(&home, dry_run);
    removed_any |= remove_project_agent_files(dry_run);

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
                "  The above changes WOULD be applied.\n  Run `lean-ctx {mode_label}` to execute.\n"
            );
        } else if keep_config {
            println!(
                "  Runtime data removed. MCP configs preserved for reinstall.\n  \
                 Reinstall with: cargo install lean-ctx\n"
            );
        } else {
            println!(
                "  lean-ctx fully removed. Restart your shell to drop stale aliases.\n  \
                 Verify with: command -v lean-ctx   # should print nothing\n"
            );
        }
    } else {
        println!("  Nothing to remove — lean-ctx was not configured.\n");
    }
}

// ---------------------------------------------------------------------------
// Data directory
// ---------------------------------------------------------------------------

/// Every lean-ctx directory an uninstall must delete, de-duplicated and
/// order-preserving.
///
/// Historically this used `dirs::data_dir()` / `dirs::data_local_dir()`, which on
/// macOS both resolve to `~/Library/Application Support` — so the *real* runtime
/// dirs (`~/.local/share`, `~/.local/state`, `~/.cache`) were never removed and a
/// "full" uninstall left >150 MB of data + cache behind. We now resolve through the
/// exact same [`core::paths`](crate::core::paths) functions the proxy uses,
/// so every XDG category (config/data/state/cache, honoring `LEAN_CTX_*_DIR` and
/// `XDG_*` overrides) is covered.
fn data_dirs_to_remove(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".config/lean-ctx")];

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

    let dirs_to_remove = data_dirs_to_remove(home);

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

    // Project-local .lean-ctx/ and .lean-ctx-id in CWD
    if let Ok(cwd) = std::env::current_dir() {
        let project_dir = cwd.join(".lean-ctx");
        let project_id = cwd.join(".lean-ctx-id");
        for p in [&project_dir, &project_id] {
            if p.exists() {
                if dry_run {
                    println!("  Would remove {}", p.display());
                    removed = true;
                } else if p.is_dir() {
                    if fs::remove_dir_all(p).is_ok() {
                        println!("  ✓ Removed {}", p.display());
                        removed = true;
                    }
                } else if fs::remove_file(p).is_ok() {
                    println!("  ✓ Removed {}", p.display());
                    removed = true;
                }
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
        .args(["mcp", "remove", "lean-ctx", "--scope", "user"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match result {
        Ok(s) if s.success() => println!("  ✓ Removed lean-ctx from Claude MCP registry"),
        _ => {} // claude CLI not available or already removed
    }
}

// ---------------------------------------------------------------------------
// .bak cleanup: remove orphaned backup files after successful surgical removal
// ---------------------------------------------------------------------------

/// Every directory the installer may have written backups or files into:
/// agent config roots, their well-known subdirectories, and the project-local
/// config dirs in CWD.
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
        home.join(".github"),
    ];

    // Installers write into well-known subdirectories (hook scripts, rules
    // files, …). read_dir below is non-recursive, so backups in
    // those subdirectories were previously missed (GL #558).
    const KNOWN_SUBDIRS: [&str; 5] = ["hooks", "rules", "skills", "settings", "User"];
    let mut dirs_to_scan: Vec<PathBuf> = Vec::with_capacity(base_dirs.len() * 4);
    for dir in base_dirs {
        for sub in KNOWN_SUBDIRS {
            let p = dir.join(sub);
            if p.is_dir() {
                dirs_to_scan.push(p);
            }
        }
        dirs_to_scan.push(dir);
    }

    // Project-local config dirs in CWD get the same backup treatment as HOME:
    // setup writes (and uninstall removes) rules/hooks there too.
    if let Ok(cwd) = std::env::current_dir() {
        for rel in [
            ".cursor/rules",
            ".claude",
            ".claude/rules",
            ".claude/hooks",
            ".github",
            ".github/hooks",
            ".vscode",
        ] {
            let p = cwd.join(rel);
            if p.is_dir() {
                dirs_to_scan.push(p);
            }
        }
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
                if name_str.ends_with(".lean-ctx.tmp") {
                    let _ = fs::remove_file(entry.path());
                    cleaned += 1;
                    continue;
                }
                // Backups of our own hook scripts / rules files
                // (lean-ctx-rewrite.sh.bak, lean-ctx.mdc.bak, …): the originals
                // are lean-ctx-owned and already removed at this point, so the
                // backups are pure leftovers.
                if name_str.ends_with(".bak")
                    && (name_str.starts_with("lean-ctx-") || name_str.starts_with("lean-ctx."))
                {
                    let _ = fs::remove_file(entry.path());
                    cleaned += 1;
                    continue;
                }
                if name_str.contains(".lean-ctx.invalid.") && name_str.ends_with(".bak") {
                    let _ = fs::remove_file(entry.path());
                    cleaned += 1;
                    continue;
                }
                if name_str.ends_with(".lean-ctx.bak") {
                    let original_name = name_str.trim_end_matches(".lean-ctx.bak");
                    let original = entry.path().with_file_name(original_name);
                    if original.exists() {
                        match fs::read_to_string(&original) {
                            Ok(c) if !c.contains("lean-ctx") => {
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
                // lean-ctx content, its backup would otherwise be orphaned.
                if name_str.ends_with(".bak")
                    && !name_str.contains(".lean-ctx")
                    && let Ok(bak_content) = fs::read_to_string(entry.path())
                    && bak_content.contains("lean-ctx")
                {
                    let _ = fs::remove_file(entry.path());
                    cleaned += 1;
                }
            }
        }
    }

    // Also clean shell RC backups
    let rc_baks = [
        home.join(".zshrc.lean-ctx.bak"),
        home.join(".zshenv.lean-ctx.bak"),
        home.join(".bashrc.lean-ctx.bak"),
        home.join(".bashenv.lean-ctx.bak"),
    ];
    for bak in &rc_baks {
        if bak.exists() {
            let original_name = bak
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .trim_end_matches(".lean-ctx.bak")
                .to_string();
            let original = bak.with_file_name(original_name);
            if original.exists() {
                if let Ok(c) = fs::read_to_string(&original)
                    && !c.contains("lean-ctx")
                {
                    let _ = fs::remove_file(bak);
                    cleaned += 1;
                }
            } else {
                let _ = fs::remove_file(bak);
                cleaned += 1;
            }
        }
    }

    if cleaned > 0 {
        println!("  ✓ Cleaned up {cleaned} backup file(s)");
    }
}

/// Sweep now-empty installer-created directories (hooks/, rules/, skills/,
/// steering/). `fs::remove_dir` refuses to delete non-empty directories, so
/// anything still holding user content survives untouched. Runs as the last
/// filesystem step of `run()` — after every file-removal pass has finished.
fn sweep_empty_installer_dirs(home: &Path) {
    let mut swept = 0;
    for dir in scan_dirs(home) {
        let is_installer_dir = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| matches!(n, "hooks" | "rules" | "skills" | "steering"));
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
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/tester"));
        let dirs = data_dirs_to_remove(&home);

        assert!(dirs.contains(&home.join(".config/lean-ctx")));

        // Regression guard for the macOS data/state/cache leak (#uninstall-completeness):
        // the set MUST include whatever core::paths actually resolves for every XDG
        // category. The old dirs::data_dir()-based code missed these — on macOS they
        // collapse onto Application Support — leaving the real ~/.local/share +
        // ~/.local/state + ~/.cache (>150 MB) behind after a "full" uninstall.
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
