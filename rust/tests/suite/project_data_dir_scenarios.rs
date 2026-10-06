//! Scenario tests for project-scoped data directory isolation.
//!
//! Validates that project-scoped files (overlays.json, policies.json, proofs/)
//! are never written into the global data directory (~/.lean-ctx/).
//! Covers cowwoc's report: overlays.json appearing in ~/.lean-ctx/ when project
//! root resolves to the home directory.

use std::path::Path;

use lean_ctx::core::pathutil::{
    is_broad_or_unsafe_root, is_data_dir_collision, safe_project_data_dir,
};

// ---------------------------------------------------------------------------
// is_broad_or_unsafe_root
// ---------------------------------------------------------------------------

#[test]
fn backslash_root_is_broad() {
    assert!(is_broad_or_unsafe_root(Path::new("\\")));
}

#[test]
fn claude_sandbox_dir_is_broad() {
    assert!(is_broad_or_unsafe_root(Path::new("/home/user/.claude")));
    assert!(is_broad_or_unsafe_root(Path::new(
        "/Users/dev/.claude/projects/abc"
    )));
}

#[test]
fn codex_sandbox_dir_is_broad() {
    assert!(is_broad_or_unsafe_root(Path::new("/home/user/.codex")));
    assert!(is_broad_or_unsafe_root(Path::new(
        "/home/user/.codex/sandbox"
    )));
}

#[test]
fn tmp_subdir_is_not_broad() {
    assert!(!is_broad_or_unsafe_root(Path::new("/tmp/my-project")));
}

// ---------------------------------------------------------------------------
// is_data_dir_collision
// ---------------------------------------------------------------------------

#[test]
fn collision_rejects_home_dir() {
    if let Some(home) = dirs::home_dir() {
        assert!(
            is_data_dir_collision(&home),
            "home dir as project root must be a data dir collision"
        );
    }
}

#[test]
fn collision_rejects_filesystem_root() {
    assert!(
        is_data_dir_collision(Path::new("/")),
        "/ as project root must be a collision"
    );
}

#[test]
fn collision_allows_normal_project() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("safe-project");
    std::fs::create_dir_all(&project).unwrap();
    assert!(
        !is_data_dir_collision(&project),
        "normal project should not collide with data dir"
    );
}

// ---------------------------------------------------------------------------
// safe_project_data_dir
// ---------------------------------------------------------------------------

#[test]
fn safe_dir_returns_err_for_home() {
    if let Some(home) = dirs::home_dir() {
        let result = safe_project_data_dir(&home);
        assert!(result.is_err(), "home dir should produce Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("collides"),
            "error message should mention collision: {msg}"
        );
    }
}

#[test]
fn safe_dir_returns_err_for_root() {
    let result = safe_project_data_dir(Path::new("/"));
    assert!(result.is_err());
}

#[test]
fn safe_dir_returns_ok_for_normal_project() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("good-project");
    std::fs::create_dir_all(&project).unwrap();

    let result = safe_project_data_dir(&project);
    assert!(result.is_ok());
    let dir = result.unwrap();
    assert_eq!(dir, project.join(".lean-ctx"));
}

// ---------------------------------------------------------------------------
// Proof writers: safe_project_data_dir gate
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Windows-specific edge cases (path format variants)
// ---------------------------------------------------------------------------

#[test]
fn agent_sandbox_with_subpath_is_broad() {
    assert!(is_broad_or_unsafe_root(Path::new(
        "/home/cowwoc/.claude/projects/myapp"
    )));
    assert!(is_broad_or_unsafe_root(Path::new(
        "/home/cowwoc/.codex/workspace"
    )));
}

// ---------------------------------------------------------------------------
// Data dir collision: direct collision check
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// find_project_root: CWD fallback guard
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Combined scenario: cowwoc's exact setup
// ---------------------------------------------------------------------------

#[test]
fn cowwoc_scenario_home_as_project_root() {
    if let Some(home) = dirs::home_dir() {
        let safe_dir_result = safe_project_data_dir(&home);
        assert!(
            safe_dir_result.is_err(),
            "cowwoc scenario: proof write to ~ should be blocked"
        );
    }
}
