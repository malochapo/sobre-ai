//! Root guard for walks nobody asked for by name: a tool called without a
//! `path` walks the project root, and the session overview and `ctx_compose`
//! walk it on their own.
//!
//! The MCP server is often spawned with `cwd == $HOME` (Cursor starts
//! user-level servers from the home directory), so that root can be the whole
//! home directory. Walking it stats everything under `~/Library`, `~/Desktop`,
//! … (a macOS TCC privacy prompt each, the #356 class) and hydrates cloud
//! placeholders on Windows (#363). A path the agent names is always walked:
//! that choice is the agent's (invariant 5).

/// Returns an actionable `ERROR:` message when `dir` must not be walked,
/// or `None` when the walk is safe to proceed.
pub(crate) fn deny_unsafe_walk_root(dir: &str) -> Option<String> {
    let abs = std::path::absolute(dir)
        .map_or_else(|_| dir.to_string(), |p| p.to_string_lossy().to_string());
    if crate::core::graph_index::is_safe_scan_root_public(&abs) {
        return None;
    }
    Some(format!(
        "ERROR: refusing to scan '{dir}' — it resolves to a broad or privacy-protected directory ({abs}). Pass a specific project directory as `path`."
    ))
}

#[cfg(test)]
mod tests {
    use super::deny_unsafe_walk_root;

    #[test]
    fn refuses_filesystem_root() {
        let msg = deny_unsafe_walk_root("/").expect("filesystem root must be refused");
        assert!(msg.starts_with("ERROR:"), "actionable error: {msg}");
    }

    #[test]
    fn refuses_home_directory() {
        let home = dirs::home_dir().expect("home dir in test env");
        let msg =
            deny_unsafe_walk_root(&home.to_string_lossy()).expect("home directory must be refused");
        assert!(msg.contains("privacy-protected") || msg.contains("broad"));
    }

    #[test]
    fn allows_plain_temp_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            deny_unsafe_walk_root(&dir.path().to_string_lossy()),
            None,
            "temp project dirs stay walkable"
        );
    }

    #[test]
    fn relative_dot_is_absolutized_not_blanket_refused() {
        // The test process runs inside the sobre repo (a real project with
        // markers), so "." must absolutize to a safe root.
        assert_eq!(deny_unsafe_walk_root("."), None);
    }
}
