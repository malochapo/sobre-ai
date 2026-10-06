//! Scenario tests for project-scoped data directory isolation.
//!
//! Validates which project roots are too broad to hold project-scoped data
//! (home, filesystem root, agent sandboxes).

use std::path::Path;

use lean_ctx::core::pathutil::is_broad_or_unsafe_root;

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

// ---------------------------------------------------------------------------
// safe_project_data_dir
// ---------------------------------------------------------------------------

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
