pub mod autonomy;
pub(crate) mod edit_io;
pub(crate) mod edit_recovery;
pub(crate) mod graph_meta;
pub mod sobre_compose;
pub mod sobre_expand;
pub mod sobre_glob;
pub mod sobre_graph;
pub mod sobre_graph_calls;
pub mod sobre_graph_diagram;
pub mod sobre_graph_impact;
pub mod sobre_graph_primitives;
pub mod sobre_knowledge;
pub mod sobre_knowledge_relations;
pub mod sobre_outline;
pub mod sobre_patch;
pub mod sobre_read;
pub mod sobre_refactor;
pub mod sobre_review;
pub mod sobre_search;
pub mod sobre_session;
pub mod sobre_shell;
pub mod sobre_tree;
// Stays `pub`: driven from outside the crate by tests/suite/property_graph_reproducibility_contract.rs.
pub(crate) mod impl_dedup;
// Stays `pub`: driven from outside the crate by tests/quality_loop_golden.rs and tests/suite/edit_reliability.rs.
pub mod impl_edit;
// Stays `pub`: driven from outside the crate by tests/serial_suite/multi_read_cap_scenarios.rs.
pub mod impl_multi_read;
pub(crate) mod impl_overview;
// Stays `pub`: driven from outside the crate by tests/serial_suite/intensive_benchmarks.rs.
pub(crate) mod impl_semantic_search;
pub(crate) mod impl_smart_read;
pub(crate) mod impl_symbol;
pub(crate) mod output_format;
pub mod registered;
pub(crate) mod walk_guard;

mod server;
mod server_lifecycle;
mod server_metrics;
mod server_paths;
pub(crate) mod startup;

pub use server::*;
pub use startup::create_server;

#[cfg(test)]
mod resolve_path_tests {
    use super::startup::canonicalize_path;
    use super::*;

    fn create_git_root(path: &std::path::Path) -> String {
        std::fs::create_dir_all(path.join(".git")).unwrap();
        canonicalize_path(path)
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resolve_path_reads_outside_trusted_startup_root_without_rerooting() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let stale = tmp.path().join("stale");
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&stale).unwrap();
        create_git_root(&root);
        let _other_value = create_git_root(&other);
        std::fs::write(other.join("b.txt"), "no").unwrap();

        let server = SobreServer::new_with_startup(None, Some(root.as_path()));
        {
            let mut session = server.session.write().await;
            session.project_root = Some(stale.to_string_lossy().to_string());
            session.shell_cwd = Some(stale.to_string_lossy().to_string());
        }

        let out = server
            .resolve_path(&other.join("b.txt").to_string_lossy())
            .await
            .unwrap();
        assert!(out.ends_with("/b.txt"), "got: {out}");

        let session = server.session.read().await;
        assert_eq!(
            session.project_root.as_deref(),
            Some(stale.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resolve_path_auto_reroots_from_agent_config_dir() {
        // The MCP server is launched from an agent/IDE config dir
        // (~/.copilot-style) and wrongly adopts it as the root. With no
        // trusted startup root, the first
        // absolute path into a real project must still correct the root — an
        // agent config dir is never a real project root.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join(".copilot");
        let real = tmp.path().join("repo");
        std::fs::create_dir_all(&agent).unwrap();
        let real_root = create_git_root(&real);
        std::fs::write(real.join("a.txt"), "ok").unwrap();

        let server = SobreServer::new_with_startup(None, None);
        {
            let mut session = server.session.write().await;
            session.project_root = Some(agent.to_string_lossy().to_string());
            session.shell_cwd = Some(agent.to_string_lossy().to_string());
        }

        let out = server
            .resolve_path(&real.join("a.txt").to_string_lossy())
            .await
            .unwrap();
        assert!(
            out.ends_with("/a.txt"),
            "agent-dir root must auto-correct: {out}"
        );

        let session = server.session.read().await;
        assert_eq!(session.project_root.as_deref(), Some(real_root.as_str()));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resolve_path_auto_reroots_from_markerless_client_cwd() {
        // VS Code/WSL can launch the MCP server from /mnt/c/Users while the
        // workspace lives under /mnt/d. That markerless cwd must not stay the
        // root when the request points at a real project.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let client_cwd = tmp.path().join("Users").join("user");
        let real = tmp.path().join("workspaces").join("sobre");
        std::fs::create_dir_all(&client_cwd).unwrap();
        let real_root = create_git_root(&real);
        std::fs::write(real.join("rust.rs"), "ok").unwrap();

        let server = SobreServer::new_with_startup(None, None);
        {
            let mut session = server.session.write().await;
            session.project_root = Some(client_cwd.to_string_lossy().to_string());
            session.shell_cwd = Some(client_cwd.to_string_lossy().to_string());
        }

        let out = server
            .resolve_path(&real.join("rust.rs").to_string_lossy())
            .await
            .unwrap();
        assert!(
            out.ends_with("/rust.rs"),
            "markerless client cwd must auto-correct: {out}"
        );

        let session = server.session.read().await;
        assert_eq!(session.project_root.as_deref(), Some(real_root.as_str()));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resolve_path_markerless_root_keeps_root_for_markerless_path() {
        // From a markerless client cwd, an absolute path that derives NO
        // project marker is read as is and the root is unchanged. Only a real
        // project becomes the new root.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let client_cwd = tmp.path().join("Users").join("user");
        let loose = tmp.path().join("workspaces").join("loose");
        std::fs::create_dir_all(&client_cwd).unwrap();
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("data.txt"), "no").unwrap();

        let server = SobreServer::new_with_startup(None, None);
        {
            let mut session = server.session.write().await;
            session.project_root = Some(client_cwd.to_string_lossy().to_string());
            session.shell_cwd = Some(client_cwd.to_string_lossy().to_string());
        }

        let out = server
            .resolve_path(&loose.join("data.txt").to_string_lossy())
            .await
            .unwrap();
        assert!(out.ends_with("/data.txt"), "got: {out}");

        let session = server.session.read().await;
        assert_eq!(
            session.project_root.as_deref(),
            Some(client_cwd.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resolve_path_agent_dir_keeps_root_for_markerless_path() {
        // The agent-dir correction only reroots to a *real* project (one
        // carrying a marker). A markerless absolute path is read as is and the
        // root is unchanged.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join(".copilot");
        let loose = tmp.path().join("loose");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("data.txt"), "no").unwrap();

        let server = SobreServer::new_with_startup(None, None);
        {
            let mut session = server.session.write().await;
            session.project_root = Some(agent.to_string_lossy().to_string());
            session.shell_cwd = Some(agent.to_string_lossy().to_string());
        }

        let out = server
            .resolve_path(&loose.join("data.txt").to_string_lossy())
            .await
            .unwrap();
        assert!(out.ends_with("/data.txt"), "got: {out}");

        let session = server.session.read().await;
        assert_eq!(
            session.project_root.as_deref(),
            Some(agent.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn startup_prefers_workspace_scoped_session_over_global_latest() {
        let _lock = crate::core::data_dir::test_env_lock();
        let _data = tempfile::tempdir().unwrap();
        let _tmp = tempfile::tempdir().unwrap();

        crate::test_env::set_var("SOBRE_DATA_DIR", _data.path());

        let repo_a = _tmp.path().join("repo-a");
        let repo_b = _tmp.path().join("repo-b");
        let root_a = create_git_root(&repo_a);
        let root_b = create_git_root(&repo_b);

        let mut session_b = crate::core::session::SessionState::new();
        session_b.project_root = Some(root_b.clone());
        session_b.shell_cwd = Some(root_b.clone());
        session_b.set_task("repo-b task", None);
        session_b.save().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut session_a = crate::core::session::SessionState::new();
        session_a.project_root = Some(root_a.clone());
        session_a.shell_cwd = Some(root_a.clone());
        session_a.set_task("repo-a latest task", None);
        session_a.save().unwrap();

        let server = SobreServer::new_with_startup(None, Some(repo_b.as_path()));
        crate::test_env::remove_var("SOBRE_DATA_DIR");

        let session = server.session.read().await;
        assert_eq!(session.project_root.as_deref(), Some(root_b.as_str()));
        assert_eq!(session.shell_cwd.as_deref(), Some(root_b.as_str()));
        assert_eq!(
            session.task.as_ref().map(|t| t.description.as_str()),
            Some("repo-b task")
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn startup_creates_fresh_session_for_new_workspace_and_preserves_subdir_cwd() {
        let _lock = crate::core::data_dir::test_env_lock();
        let _data = tempfile::tempdir().unwrap();
        let _tmp = tempfile::tempdir().unwrap();

        crate::test_env::set_var("SOBRE_DATA_DIR", _data.path());

        let repo_a = _tmp.path().join("repo-a");
        let repo_b = _tmp.path().join("repo-b");
        let repo_b_src = repo_b.join("src");
        let root_a = create_git_root(&repo_a);
        let root_b = create_git_root(&repo_b);
        std::fs::create_dir_all(&repo_b_src).unwrap();
        let repo_b_src_value = canonicalize_path(&repo_b_src);

        let mut session_a = crate::core::session::SessionState::new();
        session_a.project_root = Some(root_a.clone());
        session_a.shell_cwd = Some(root_a.clone());
        session_a.set_task("repo-a latest task", None);
        let old_id = session_a.id.clone();
        session_a.save().unwrap();

        let server = SobreServer::new_with_startup(None, Some(repo_b_src.as_path()));
        crate::test_env::remove_var("SOBRE_DATA_DIR");

        let session = server.session.read().await;
        assert_eq!(session.project_root.as_deref(), Some(root_b.as_str()));
        assert_eq!(
            session.shell_cwd.as_deref(),
            Some(repo_b_src_value.as_str())
        );
        assert!(session.task.is_none());
        assert_ne!(session.id, old_id);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resolve_path_does_not_reroot_when_current_root_is_real_project() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        let root_value = create_git_root(&root);
        create_git_root(&other);
        std::fs::write(other.join("b.txt"), "no").unwrap();

        let root_str = root.to_string_lossy().to_string();
        let server = SobreServer::new_with_project_root(Some(&root_str));

        let out = server
            .resolve_path(&other.join("b.txt").to_string_lossy())
            .await
            .unwrap();
        assert!(out.ends_with("/b.txt"), "got: {out}");

        let session = server.session.read().await;
        assert_eq!(session.project_root.as_deref(), Some(root_value.as_str()));
    }

    // A mid-session worktree switch moves shell_cwd into a nested linked
    // worktree (own `.git` *file*) while project_root stays at initialize-time.
    // The path `src/lib.rs` deliberately also exists relative to the test
    // process CWD (`rust/`): the server's `p.exists()` probe used to
    // short-circuit on exactly that and serve the stale root copy before the
    // divergent-checkout precedence could apply.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resolve_path_prefers_worktree_copy_after_mid_session_switch() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let root_value = create_git_root(&repo);
        let wt = repo.join(".claude/worktrees/wt");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        // As `git worktree add` lays it out: the worktree's entry under the
        // main repository's .git, with a commondir leading back to it.
        std::fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
        std::fs::write(repo.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), "gitdir: ../../../.git/worktrees/wt\n").unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "stale").unwrap();
        std::fs::write(wt.join("src/lib.rs"), "fresh").unwrap();

        let server = SobreServer::new_with_project_root(Some(&root_value));
        {
            // The worktree switch as sobre_shell's cwd tracking records it.
            let mut session = server.session.write().await;
            session.shell_cwd = Some(wt.to_string_lossy().to_string());
        }

        let out = server.resolve_path("src/lib.rs").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "fresh",
            "worktree copy must win over the stale project_root copy: {out}"
        );

        // Switching back restores the established precedence: without
        // divergence the project_root fallback serves the root copy again.
        // Probed via a path that does NOT exist relative to the test process
        // CWD — `src/lib.rs` would hit the (intended) `p.exists()` fast path
        // and resolve to the real checkout's file, making the assertion
        // environment-dependent (that is exactly how CI diverged from local).
        {
            let mut session = server.session.write().await;
            session.shell_cwd = Some(root_value.clone());
        }
        std::fs::write(repo.join("src/back_707.rs"), "stale").unwrap();
        std::fs::write(wt.join("src/back_707.rs"), "fresh").unwrap();
        let out = server.resolve_path("src/back_707.rs").await.unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "stale");
    }
}
