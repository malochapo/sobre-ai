//! Smoke tests: a session survives a serde round trip and builds its resume
//! block, and `sobre config` lists the keys.

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
    fn cli_config_lists_the_keys() {
        let out = sobre_bin().arg("config").output().unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("passthrough_commands"));
    }
}
