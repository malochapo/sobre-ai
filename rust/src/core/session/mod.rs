mod paths;
mod state;
mod types;

pub use types::SessionState;

#[cfg(test)]
mod tests {
    use super::paths::extract_cd_target;
    use super::types::*;

    /// An explicit `cwd` param must persist as the live
    /// shell cwd — worktree switches arrive as `cwd` params, not `cd`
    /// commands, and the divergence check in path resolution reads
    /// `shell_cwd`. Relative or nonexistent paths are ignored.
    #[test]
    fn note_explicit_cwd_persists_absolute_dirs_only() {
        let tmp = std::env::temp_dir().join(format!("sobre_cwd_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let mut session = SessionState::new();
        session.note_explicit_cwd(&tmp.to_string_lossy());
        let noted = session.shell_cwd.clone().expect("cwd should persist");
        // Canonicalized (macOS /tmp → /private/tmp), so compare canonical forms.
        assert_eq!(
            noted,
            crate::core::pathutil::safe_canonicalize_or_self(&tmp)
                .to_string_lossy()
                .to_string()
        );

        session.note_explicit_cwd("relative/dir");
        assert_eq!(session.shell_cwd.as_deref(), Some(noted.as_str()));
        session.note_explicit_cwd(&tmp.join("does-not-exist").to_string_lossy());
        assert_eq!(session.shell_cwd.as_deref(), Some(noted.as_str()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extract_cd_with_semicolon() {
        let result = extract_cd_target("cd /tmp; ls", "/home/user");
        assert_eq!(result, Some("/tmp".to_string()));
    }

    #[test]
    fn extract_cd_parent_dir() {
        let result = extract_cd_target("cd ..", "/home/user/project");
        assert_eq!(result, Some("/home/user/project/..".to_string()));
    }

    #[test]
    fn effective_cwd_explicit_takes_priority() {
        let tmp = std::env::temp_dir().join("sobre-test-cwd-explicit");
        let sub = tmp.join("sub");
        let _ = std::fs::create_dir_all(&sub);
        let root_canon = crate::core::pathutil::safe_canonicalize_or_self(&tmp)
            .to_string_lossy()
            .to_string();
        let sub_canon = crate::core::pathutil::safe_canonicalize_or_self(&sub)
            .to_string_lossy()
            .to_string();

        let mut session = SessionState::new();
        session.project_root = Some(root_canon);
        let result = session.effective_cwd(Some(&sub_canon));
        assert_eq!(result, sub_canon);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn effective_cwd_shell_cwd_second_priority() {
        let mut session = SessionState::new();
        session.project_root = Some("/project".to_string());
        session.shell_cwd = Some("/project/src".to_string());
        assert_eq!(session.effective_cwd(None), "/project/src");
    }

    #[test]
    fn effective_cwd_project_root_third_priority() {
        let mut session = SessionState::new();
        session.project_root = Some("/project".to_string());
        assert_eq!(session.effective_cwd(None), "/project");
    }

    #[test]
    fn effective_cwd_dot_ignored() {
        let mut session = SessionState::new();
        session.project_root = Some("/project".to_string());
        assert_eq!(session.effective_cwd(Some(".")), "/project");
    }
}
