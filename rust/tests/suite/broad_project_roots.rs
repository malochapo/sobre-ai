//! Which project roots are too broad to hold project-scoped data: the
//! filesystem root and agent sandboxes are, an ordinary project is not.

use std::path::Path;

use sobre::core::pathutil::is_broad_or_unsafe_root;

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

#[test]
fn agent_sandbox_with_subpath_is_broad() {
    assert!(is_broad_or_unsafe_root(Path::new(
        "/home/dev/.claude/projects/myapp"
    )));
    assert!(is_broad_or_unsafe_root(Path::new(
        "/home/dev/.codex/workspace"
    )));
}
