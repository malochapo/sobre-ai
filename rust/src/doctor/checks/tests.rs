use super::*;
use std::path::Path;

fn write(home: &Path, rel: &str, content: &str) {
    let p = home.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn check(home: &Path) -> Outcome {
    claude_instructions_check(home)
}

#[test]
fn recent_sessions_line_names_each_workspace_once_with_age() {
    use crate::core::session::SessionSummary;
    let now = chrono::Utc::now();
    let mk = |root: Option<&str>, mins_ago: i64| SessionSummary {
        id: format!("s{mins_ago}"),
        started_at: now - chrono::Duration::minutes(mins_ago + 5),
        updated_at: now - chrono::Duration::minutes(mins_ago),
        version: 1,
        task: None,
        tool_calls: 3,
        tokens_saved: 100,
        project_root: root.map(str::to_string),
    };

    // Two windows on two projects + a stale duplicate of the first root +
    // a rootless session that must be skipped.
    let line = environment::format_recent_sessions(
        vec![
            mk(Some("/Users/me/work/frontend"), 4),
            mk(Some("/Users/me/work/backend"), 90),
            mk(Some("/Users/me/work/frontend"), 200),
            mk(None, 1),
        ],
        now,
    )
    .expect("sessions exist");

    assert_eq!(line, "recent: frontend (4m ago), backend (1h ago)");
}

#[test]
fn recent_sessions_line_is_none_without_any_rooted_session() {
    let now = chrono::Utc::now();
    assert_eq!(environment::format_recent_sessions(vec![], now), None);
}

#[test]
fn humanized_ages_cover_all_magnitudes() {
    use chrono::Duration;
    assert_eq!(environment::humanize_age(Duration::seconds(20)), "just now");
    assert_eq!(environment::humanize_age(Duration::minutes(59)), "59m ago");
    assert_eq!(environment::humanize_age(Duration::hours(47)), "47h ago");
    assert_eq!(environment::humanize_age(Duration::days(3)), "3d ago");
}

#[test]
fn capacity_hint_is_actionable_for_both_states() {
    // WARN (at/near cap): reassure it is by-design, point at the cap lever.
    let warn = capacity_hint(false);
    assert!(warn.contains("healthy by design"));
    assert!(warn.contains("memory.*"));

    // CRIT (over cap): give an immediate compaction action.
    let crit = capacity_hint(true);
    assert!(crit.contains("lean-ctx knowledge consolidate --all"));
    assert!(crit.contains("memory.*"));

    assert_ne!(warn, crit);
}

#[test]
fn cwd_looks_like_agent_dir_matches_both_separators() {
    for sep in ['/', '\\'] {
        for dir in [".lmstudio", ".claude", ".codebuddy", ".codex"] {
            let cwd = format!("C:{sep}Users{sep}me{sep}{dir}{sep}mcp");
            assert!(
                cwd_looks_like_agent_dir(&cwd),
                "expected {cwd} to be flagged as an agent dir"
            );
        }
    }
}

#[test]
fn cwd_looks_like_agent_dir_ignores_real_projects() {
    for cwd in [
        "/home/me/work/myproj",
        "/Users/me/code/lean-ctx",
        "C:\\src\\app",
    ] {
        assert!(
            !cwd_looks_like_agent_dir(cwd),
            "{cwd} is a real project and must not be flagged"
        );
    }
}

// GH #396: the exact post-`setup` state — the CLAUDE.md block, rules file
// removed by setup. Must pass, not demand the retired rules file.
//
// `serial(claude_config_dir)`: `claude_state_dir` honours the process-global
// `CLAUDE_CONFIG_DIR`, which the contextops sync tests set for their own
// sandbox. Without serialization a concurrent setter makes this check read
// the wrong `.claude` dir and flake under load (seen on release CI, #401).
#[test]
#[serial_test::serial(claude_config_dir)]
fn v3_layout_block_passes() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        ".claude/CLAUDE.md",
        &format!(
            "{}\ncontent\n{}",
            crate::core::rules_canonical::AGENTS_BLOCK_START,
            crate::core::rules_canonical::AGENTS_BLOCK_END,
        ),
    );
    let out = check(tmp.path());
    assert!(out.ok, "post-setup layout must pass: {}", out.line);
    assert!(out.line.contains("CLAUDE.md block installed"));
}

#[test]
#[serial_test::serial(claude_config_dir)]
fn block_alone_passes() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        ".claude/CLAUDE.md",
        &format!("{}\nx", crate::core::rules_canonical::AGENTS_BLOCK_START),
    );
    let out = check(tmp.path());
    assert!(out.ok, "{}", out.line);
}

#[test]
#[serial_test::serial(claude_config_dir)]
fn nothing_installed_fails_and_suggests_setup() {
    let tmp = tempfile::tempdir().unwrap();
    let out = check(tmp.path());
    assert!(!out.ok);
    assert!(
        out.line.contains("lean-ctx init"),
        "must suggest a command that actually fixes it: {}",
        out.line
    );
    assert!(
        !out.line.contains("init --agent claude"),
        "init --agent claude no longer creates a Claude rules target"
    );
}
