//! End-to-end smoke tests: session lifecycle, CLI commands, contract docs,
//! knowledge and shell compression.

use std::process::Command;

// ─── helpers ───────────────────────────────────────────────────────

fn sobre_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sobre"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd
}

mod session_features {
    use sobre::core::session::{Decision, FileTouched, Finding, SessionState, TaskInfo};

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn full_session_lifecycle() {
        let mut session = SessionState::default();
        session.project_root = Some("/home/user/project".to_string());

        session.task = Some(TaskInfo {
            description: "Build Context OS".to_string(),
            intent: Some("architecture".to_string()),
        });

        for i in 0..5 {
            session.findings.push(Finding {
                file: Some(format!("src/module{i}.rs")),
                line: Some(i * 10),
                summary: format!("Found pattern {i}"),
                timestamp: chrono::Utc::now(),
            });
        }

        session.decisions.push(Decision {
            summary: "Use SSE for realtime events".to_string(),
            rationale: Some("Lower overhead than WebSocket for unidirectional flow".to_string()),
            timestamp: chrono::Utc::now(),
        });

        for i in 0..10 {
            session.files_touched.push(FileTouched {
                path: format!("src/file{i}.rs"),
                file_ref: None,
                read_count: i as u32 + 1,
                modified: i % 2 == 0,
                last_mode: "full".to_string(),
                tokens: 500,
                stale: false,
                summary: None,
            });
        }

        let resume = session.build_resume_block();
        assert!(resume.contains("Build Context OS"), "task in resume");
        assert!(resume.contains("SSE"), "decision in resume");

        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.findings.len(), 5);
        assert_eq!(restored.files_touched.len(), 10);
    }
}

mod cli_commands {
    use super::*;

    #[test]
    fn cli_help() {
        let out = sobre_bin().arg("--help").output().unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("local context tools for coding agents"));
    }

    #[test]
    fn cli_config() {
        let out = sobre_bin().arg("config").output().unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("passthrough_commands"));
    }

    #[test]
    fn cli_shell_echo() {
        let out = sobre_bin().args(["-c", "echo hello"]).output().unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("hello"));
    }

    #[test]
    fn cli_doctor() {
        let out = sobre_bin().arg("doctor").output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("sobre") || combined.contains("doctor") || combined.contains("check"),
            "doctor must produce diagnostic output"
        );
    }
}

mod contract_gates {

    #[test]
    fn committed_contract_docs_are_valid() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let committed = vec!["usage-ledger-v1.md"];

        for name in &committed {
            let path = format!("{manifest_dir}/../docs/contracts/{name}");
            assert!(
                std::path::Path::new(&path).exists(),
                "committed contract doc {name} must exist"
            );
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(content.len() > 200, "{name} must have substantial content");
        }
    }
}

mod knowledge_system {
    use sobre::core::knowledge::ProjectKnowledge;
    use sobre::core::memory_policy::MemoryPolicy;

    #[test]
    fn remember_recall_roundtrip() {
        let policy = MemoryPolicy::default();
        let mut kb = ProjectKnowledge::new("/tmp/knowledge-test-ultimate");
        kb.remember(
            "architecture",
            "auth_jwt",
            "Auth uses JWT tokens",
            "test-session",
            0.9,
            &policy,
        );
        kb.remember(
            "architecture",
            "db_postgres",
            "Database is PostgreSQL",
            "test-session",
            0.9,
            &policy,
        );
        kb.remember(
            "testing",
            "prop_based",
            "Use property-based testing for parsers",
            "test-session",
            0.8,
            &policy,
        );

        let arch_facts = kb.recall_by_category("architecture");
        assert!(arch_facts.len() >= 2, "must recall architecture facts");

        let query_results = kb.recall("JWT");
        assert!(
            !query_results.is_empty(),
            "must find JWT-related facts by query"
        );
    }

    #[test]
    fn knowledge_multi_category_recall() {
        let policy = MemoryPolicy::default();
        let mut kb = ProjectKnowledge::new("/tmp/knowledge-multi-cat-ultimate");
        kb.remember(
            "security",
            "cors_policy",
            "CORS allows only trusted origins",
            "s1",
            0.9,
            &policy,
        );
        kb.remember(
            "security",
            "auth_flow",
            "OAuth2 with PKCE for public clients",
            "s1",
            0.9,
            &policy,
        );
        kb.remember(
            "performance",
            "caching",
            "Redis cache with 5min TTL",
            "s1",
            0.8,
            &policy,
        );

        let security = kb.recall_by_category("security");
        assert_eq!(security.len(), 2, "2 security facts");

        let perf = kb.recall_by_category("performance");
        assert_eq!(perf.len(), 1, "1 performance fact");

        let empty = kb.recall_by_category("nonexistent");
        assert!(empty.is_empty(), "no facts for nonexistent category");
    }
}

mod shell_compression {
    use super::*;

    #[test]
    fn shell_compresses_git_status() {
        let out = sobre_bin().args(["-c", "git status"]).output().unwrap();
        assert!(out.status.success());
    }

    #[test]
    fn shell_compresses_cargo_build_output() {
        let out = sobre_bin()
            .args(["-c", "echo 'Compiling sobre v3.4.7\n   Compiling serde v1.0.0\n   Compiling tokio v1.0.0\n   Finished dev [unoptimized + debuginfo] target(s) in 5.00s'"])
            .output().unwrap();
        assert!(out.status.success());
    }

    #[test]
    fn shell_compresses_ls_output() {
        // `ls -la` is not a PowerShell or cmd command; `dir` lists a directory
        // under all three shells Windows may resolve to (PowerShell, cmd, Git Bash).
        let listing = if cfg!(windows) { "dir" } else { "ls -la" };
        let out = sobre_bin().args(["-c", listing]).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
