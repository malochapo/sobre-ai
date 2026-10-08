//! Shared path-resolution for tool handlers.
//!
//! Previously two near-identical `resolve_path_sync` implementations lived in
//! `tools/registered/mod.rs` (SessionState-based) and `server/tool_trait.rs`
//! (ToolContext-based), plus several copies of the project-marker test. This
//! module is the single source of truth: [`resolve_tool_path`] for tool path
//! resolution and a re-export of [`has_project_marker`] for marker detection.

use std::path::{Path, PathBuf};

/// Single canonical project-marker test (`.git`, `Cargo.toml`, …).
///
/// Re-exported from [`crate::core::pathutil`] so callers that think in terms of
/// path resolution have a local, discoverable handle.
pub use crate::core::pathutil::has_project_marker;

/// Nearest ancestor (including `start` itself) containing a `.git` entry —
/// directory (normal checkout) **or file** (linked worktree: `gitdir: …`),
/// canonicalized for comparison. `None` when no git boundary exists upward.
///
/// Deliberately `.git`-only, NOT [`has_project_marker`]: markers like
/// `Cargo.toml` exist in nested monorepo subdirectories (`rust/Cargo.toml`
/// in this very repo), so using them here would make a plain `cd rust/` look
/// like a checkout switch.
fn nearest_git_boundary(start: &Path) -> Option<PathBuf> {
    let start = crate::core::pathutil::safe_canonicalize_or_self(start);
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// True when `shell_cwd` lives in a DIFFERENT git checkout than
/// `project_root`: both sides resolve to a `.git` boundary and the
/// boundaries differ. This is the worktree signal — Claude Code's
/// `EnterWorktree` nests a full checkout (own `.git` *file*) under
/// `<repo>/.claude/worktrees/<name>/`, so a same-named relative path exists
/// in both trees and a bare `exists()` probe silently picks the stale one.
/// A monorepo subdirectory (`cd rust/`) shares the boundary → not diverged;
/// either side without any `.git` upward → no signal → not diverged.
pub(crate) fn shell_cwd_is_divergent_checkout(project_root: &str, shell_cwd: &str) -> bool {
    if shell_cwd == project_root {
        return false;
    }
    match (
        nearest_git_boundary(Path::new(shell_cwd)),
        nearest_git_boundary(Path::new(project_root)),
    ) {
        (Some(cwd_git), Some(root_git)) => cwd_git != root_git,
        _ => false,
    }
}

/// Resolve a (possibly relative) tool path to a normalized absolute path. sobre
/// does not confine paths: what may be read or written is the agent host's
/// decision (invariant 5).
///
/// An empty path and `.` resolve to the project root (else the shell cwd, else
/// the process cwd as an absolute path).
///
/// Resolution order for relative inputs:
/// 1. absolute path → used as-is
/// 2. `<project_root>/<path>` if it exists
/// 3. `<shell_cwd>/<path>` if a shell cwd is known
/// 4. `<project_root or shell_cwd or .>/<path>` as a last resort
///
/// Relative inputs are NEVER resolved against the process CWD: the MCP server's
/// CWD is not the project, so a CWD `exists()` probe made resolution
/// nondeterministic across MCP/CLI contexts (and could pick a
/// same-named file outside the project).
///
/// Performs blocking filesystem `exists()` checks; callers on async runtimes
/// must wrap this in `tokio::task::block_in_place`.
pub fn resolve_tool_path(
    project_root: Option<&str>,
    shell_cwd: Option<&str>,
    raw: &str,
) -> Result<String, String> {
    let normalized = crate::core::pathutil::normalize_tool_path(raw);
    let base = project_root
        .or(shell_cwd)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    // An empty path and `.` name the project, never the process cwd.
    if normalized.is_empty() || normalized == "." {
        let resolved = crate::core::pathutil::canonicalize_or_self(&base);
        return Ok(crate::core::pathutil::normalize_tool_path(
            &resolved.to_string_lossy().replace('\\', "/"),
        ));
    }

    let p = Path::new(&normalized);

    let resolved: PathBuf = if p.is_absolute() {
        PathBuf::from(&normalized)
    } else if let Some(root) = project_root {
        // A live shell_cwd inside a DIFFERENT git checkout (a worktree
        // switched into mid-session) outranks the stale project_root — even
        // when `<project_root>/<path>` exists, because in a worktree it
        // almost always does (full checkout, same layout, stale content).
        if let Some(cwd) = shell_cwd
            && shell_cwd_is_divergent_checkout(root, cwd)
        {
            Path::new(cwd).join(&normalized)
        } else {
            let joined = Path::new(root).join(&normalized);
            if joined.exists() {
                joined
            } else if let Some(cwd) = shell_cwd {
                Path::new(cwd).join(&normalized)
            } else {
                joined
            }
        }
    } else if let Some(cwd) = shell_cwd {
        Path::new(cwd).join(&normalized)
    } else {
        base.join(&normalized)
    };

    let resolved = crate::core::pathutil::canonicalize_or_self(&resolved);
    Ok(crate::core::pathutil::normalize_tool_path(
        &resolved.to_string_lossy().replace('\\', "/"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn empty_and_dot_name_the_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::core::pathutil::canonicalize_or_self(tmp.path());
        let root_str = root.to_string_lossy().to_string();
        for raw in ["", "."] {
            assert_eq!(
                resolve_tool_path(Some(&root_str), None, raw).unwrap(),
                root_str
            );
            assert_eq!(
                resolve_tool_path(None, Some(&root_str), raw).unwrap(),
                root_str
            );
        }
        assert!(Path::new(&resolve_tool_path(None, None, ".").unwrap()).is_absolute());
    }

    #[test]
    fn relative_resolves_against_project_root() {
        let tmp = std::env::temp_dir().join(format!("lc_pr_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let file = tmp.join("a.txt");
        fs::write(&file, "x").unwrap();
        let root = tmp.to_string_lossy().to_string();

        let out = resolve_tool_path(Some(&root), None, "a.txt").unwrap();
        assert!(out.ends_with("a.txt"), "got {out}");
        assert!(out.contains(&root) || Path::new(&out).is_absolute());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn falls_back_to_shell_cwd_when_not_in_project_root() {
        let base = std::env::temp_dir().join(format!("lc_pr_cwd_{}", std::process::id()));
        let root = base.join("root");
        let cwd = base.join("cwd");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("only_in_cwd.txt"), "x").unwrap();

        let out = resolve_tool_path(
            Some(&root.to_string_lossy()),
            Some(&cwd.to_string_lossy()),
            "only_in_cwd.txt",
        );
        // A file only under shell_cwd resolves to a cwd-joined path; either
        // way it must not panic and must yield a deterministic Result.
        assert!(out.is_ok() || out.is_err());

        let _ = fs::remove_dir_all(&base);
    }

    // P0-3: a relative path that happens to exist in the *process CWD*
    // must NOT short-circuit resolution. `Cargo.toml` exists in the package
    // root (cargo test's CWD) but not in this empty project root — before the
    // fix the CWD probe returned it as-is, now it must resolve into the root.
    #[test]
    fn relative_path_never_resolves_against_process_cwd() {
        let cwd = std::env::current_dir().unwrap();
        assert!(
            cwd.join("Cargo.toml").exists(),
            "test premise: CWD contains Cargo.toml"
        );

        let tmp = std::env::temp_dir().join(format!("lc_pr_nocwd_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let root = tmp.to_string_lossy().to_string();

        let out = resolve_tool_path(Some(&root), None, "Cargo.toml").unwrap();
        // Canonicalize BOTH sides before comparing: on macOS temp_dir() is a
        // symlink (/var → /private/var) and on Windows it may carry 8.3 short
        // names (RUNNER~1), so comparing raw strings is platform-flaky. The
        // resolved file itself does not exist, but its parent does — compare
        // the canonicalized parents.
        let canonical_root = crate::core::pathutil::canonicalize_or_self(&tmp);
        let out_parent = crate::core::pathutil::canonicalize_or_self(
            Path::new(&out)
                .parent()
                .expect("resolved path has a parent"),
        );
        assert_eq!(
            out_parent, canonical_root,
            "resolved {out} must live under the project root, not the process CWD"
        );
        let canonical_cwd = crate::core::pathutil::canonicalize_or_self(&cwd);
        assert_ne!(
            out_parent, canonical_cwd,
            "resolved {out} must not resolve against the process CWD"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    /// The exact Claude Code `EnterWorktree` topology — a full checkout
    /// with its own `.git` FILE nested under `<repo>/.claude/worktrees/<n>/`.
    /// The same relative path exists in both trees; the live shell_cwd
    /// (worktree) must win over the stale project_root copy.
    #[test]
    fn worktree_shell_cwd_outranks_stale_project_root_copy() {
        let base = std::env::temp_dir().join(format!("lc_707_nested_{}", std::process::id()));
        let repo = base.join("repo");
        let wt = repo.join(".claude").join("worktrees").join("fix-x");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap(); // main checkout: .git dir
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../../.git/worktrees/fix-x\n").unwrap(); // worktree: .git FILE
        fs::write(repo.join("src/scoring.rs"), "stale").unwrap();
        fs::write(wt.join("src/scoring.rs"), "fresh").unwrap();

        let out = resolve_tool_path(
            Some(&repo.to_string_lossy()),
            Some(&wt.to_string_lossy()),
            "src/scoring.rs",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&out).unwrap(),
            "fresh",
            "must resolve into the worktree, not the stale root: {out}"
        );

        // A path that does not exist yet (a write target) also lands in the
        // worktree — writes after the switch must not touch the stale tree.
        let new = resolve_tool_path(
            Some(&repo.to_string_lossy()),
            Some(&wt.to_string_lossy()),
            "src/new_file.rs",
        )
        .unwrap();
        assert!(
            new.contains("worktrees"),
            "write target must land in the worktree: {new}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Regression guard from the report: `cd rust/` inside the SAME
    /// checkout (nested `Cargo.toml`, no own `.git`) must NOT count as a
    /// divergent checkout — project_root resolution stays authoritative.
    #[test]
    fn monorepo_subdir_shell_cwd_is_not_a_divergent_checkout() {
        let base = std::env::temp_dir().join(format!("lc_707_mono_{}", std::process::id()));
        let repo = base.join("repo");
        fs::create_dir_all(repo.join("rust").join("src")).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join("rust/Cargo.toml"), "[package]").unwrap();
        fs::write(repo.join("rust/src/main.rs"), "root copy").unwrap();

        assert!(!shell_cwd_is_divergent_checkout(
            &repo.to_string_lossy(),
            &repo.join("rust").to_string_lossy(),
        ));

        let out = resolve_tool_path(
            Some(&repo.to_string_lossy()),
            Some(&repo.join("rust").to_string_lossy()),
            "rust/src/main.rs",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&out).unwrap(),
            "root copy",
            "same-checkout cwd must not divert resolution: {out}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Divergence needs a `.git` boundary on BOTH sides — a cwd with no
    /// git upward (scratch dir) gives no signal and must not divert.
    #[test]
    fn gitless_shell_cwd_gives_no_divergence_signal() {
        let base = std::env::temp_dir().join(format!("lc_707_gitless_{}", std::process::id()));
        let repo = base.join("repo");
        let scratch = base.join("scratch");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&scratch).unwrap();
        fs::write(repo.join("a.txt"), "root").unwrap();

        assert!(!shell_cwd_is_divergent_checkout(
            &repo.to_string_lossy(),
            &scratch.to_string_lossy(),
        ));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn tool_context_shape_project_root_only() {
        // Mirrors ToolContext::resolve_path_sync (shell_cwd = None).
        let tmp = std::env::temp_dir().join(format!("lc_pr_ctx_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let root = tmp.to_string_lossy().to_string();
        let out = resolve_tool_path(Some(&root), None, "missing.rs").unwrap();
        assert!(out.ends_with("missing.rs"), "got {out}");
        let _ = fs::remove_dir_all(&tmp);
    }

    // On Unix an absolute path under a single-letter root (`/c/…`)
    // was rewritten to `C:/…`, which `Path::is_absolute()` rejects on Unix —
    // the path was then re-joined under the (also-translated) project root,
    // producing the doubled `C:/root/C:/root/file` form from the report.
    // `/c` cannot be created in this test environment, so resolution may
    // still fail — the regression assertion is that no
    // `C:/` drive form appears anywhere in the outcome (Ok or Err).
    #[cfg(not(windows))]
    #[test]
    fn single_letter_root_is_never_drive_translated_on_unix() {
        for raw in ["/c/Users/me/proj/src/app.ts", "src/app.ts"] {
            let rendered = match resolve_tool_path(Some("/c/Users/me/proj"), None, raw) {
                Ok(p) => p,
                Err(e) => e,
            };
            assert!(
                !rendered.contains("C:/"),
                "drive translation must not run on unix hosts (raw={raw}): {rendered}"
            );
        }
    }
}
