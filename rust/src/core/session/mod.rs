mod compaction;
mod heuristics;
mod paths;
mod persistence;
mod state;
mod types;

pub use types::{
    Decision, FileTouched, Finding, SessionState, SessionStats, SessionSummary, TaskInfo,
};

#[cfg(test)]
mod tests {
    use super::paths::extract_cd_target;
    use super::types::*;

    #[test]
    fn load_latest_for_broad_root_returns_none_without_scanning() {
        // The MCP server boots with cwd "/" and the dispatcher passes that as the
        // project root. Broad roots must bail out before walking the session
        // store — stat-ing persisted roots under ~/Documents from a
        // launchd-owned process pops the macOS TCC prompt.
        assert!(SessionState::load_latest_for_project_root("/").is_none());
        if let Some(home) = dirs::home_dir() {
            let home = home.to_string_lossy().to_string();
            assert!(SessionState::load_latest_for_project_root(&home).is_none());
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[serial_test::serial]
    fn normalize_session_skips_marker_probe_for_real_roots() {
        // Both guards are needed: `#[serial]` and `test_env_lock` are separate
        // mutexes, and the `SOBRE_TCC_STANDALONE` this test relies on is read
        // by every path normalization, including in tests that take only the lock.
        let _env_lock = crate::core::data_dir::test_env_lock();
        // A session whose project_root is a plausible real project must not be
        // marker-probed at load time when the process is TCC-standalone: the
        // probe itself would trip the privacy prompt. The repair
        // heuristic only ever fires for agent/temp roots.
        crate::test_env::set_var("SOBRE_TCC_STANDALONE", "1");
        let mut session = SessionState::new();
        let docs_root = dirs::home_dir()
            .unwrap_or_default()
            .join("Documents/some-project")
            .to_string_lossy()
            .to_string();
        session.project_root = Some(docs_root.clone());
        session.shell_cwd = Some(docs_root.clone());
        let normalized = super::heuristics::normalize_loaded_session(session);
        // Root is not an agent/temp dir → kept as-is, no probe needed.
        assert_eq!(normalized.project_root.as_deref(), Some(docs_root.as_str()));
        crate::test_env::remove_var("SOBRE_TCC_STANDALONE");
    }

    /// An explicit `cwd` param must persist as the live
    /// shell cwd — worktree switches arrive as `cwd` params, not `cd`
    /// commands, and the divergence check in path resolution reads
    /// `shell_cwd`. Relative or nonexistent paths are ignored.
    #[test]
    fn note_explicit_cwd_persists_absolute_dirs_only() {
        let tmp = std::env::temp_dir().join(format!("lc_707_cwd_{}", std::process::id()));
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

    /// The 5-change batch alone kept a slowly-ticking session off disk for
    /// a long time. First change on a fresh in-memory session flushes
    /// immediately; afterwards the time-based trigger fires once a change is
    /// older than SESSION_FLUSH_INTERVAL, and the batch threshold still wins.
    #[test]
    fn should_save_flushes_first_change_and_stale_changes() {
        use super::state::SESSION_FLUSH_INTERVAL;
        let mut session = SessionState::new();
        assert!(!session.should_save(), "no changes → no save");

        session.increment();
        assert!(session.should_save(), "first change must flush immediately");

        session.last_flush = Some(std::time::Instant::now());
        assert!(
            !session.should_save(),
            "inside window + below batch → defer"
        );

        for _ in 0..4 {
            session.increment();
        }
        assert!(session.should_save(), "batch threshold still applies");

        session.stats.unsaved_changes = 1;
        if let Some(old) = std::time::Instant::now().checked_sub(SESSION_FLUSH_INTERVAL) {
            session.last_flush = Some(old);
            assert!(session.should_save(), "stale change must trigger flush");
        }
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

    #[test]
    fn resume_block_prefixes_compression_hint_when_enabled() {
        // The default compression_level (0.3) is the lite band.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let session = SessionState::new();
        let block = session.build_resume_block();
        assert!(block.contains("[COMPRESSION: lite]"));
    }
}
