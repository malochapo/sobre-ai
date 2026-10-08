//! Tests for the ctx_knowledge action handlers and consolidation reports.

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::core::consolidation_engine::ConsolidateOptions;
use crate::core::memory_lifecycle::LifecycleReport;

struct CurrentDirGuard {
    previous: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl CurrentDirGuard {
    fn enter(dir: &std::path::Path) -> Self {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        let lock = LOCK.get_or_init(|| std::sync::Mutex::new(()));
        let guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        Self {
            previous,
            _lock: guard,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.previous).unwrap();
    }
}

struct DataDirGuard;

impl DataDirGuard {
    fn set(path: &std::path::Path) -> Self {
        crate::test_env::set_var("SOBRE_DATA_DIR", path);
        Self
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }
}

fn report(session_id: Option<String>, session_items: usize) -> KnowledgeConsolidationReport {
    KnowledgeConsolidationReport {
        session_id,
        session_items,
        imported_decisions: session_items / 2,
        imported_findings: session_items - session_items / 2,
        facts: 7,
        active_facts: 5,
        archived_facts: 2,
        fact_capacity_target: 6,
        fact_capacity_archived: 1,
        patterns: 2,
        patterns_capacity_target: 6,
        patterns_compacted: 0,
        history: 3,
        history_capacity_target: 6,
        history_compacted: 1,
        lifecycle: LifecycleReport {
            decayed_count: 1,
            consolidated_count: 2,
            archived_count: 3,
            compacted_count: 4,
            capacity_archived: 1,
            remaining_facts: 5,
        },
        dry_run: false,
    }
}

#[test]
fn consolidation_report_marks_no_session_import() {
    let out = format_consolidation_report(&report(None, 0));

    assert!(out.contains("Session import: none (no active session)"));
    assert!(out.contains("Lifecycle: decayed 1, consolidated 2"));
}

#[test]
fn consolidation_report_includes_session_and_lifecycle_stats() {
    let out = format_consolidation_report(&report(Some("s1".to_string()), 6));

    assert!(out.contains("Session import: s1 (6 item(s))"));
    assert!(
        out.contains("Facts: 5 active, 2 archived, 7 total (target <= 6, archived-to-target 1)")
    );
    assert!(
        out.contains(
            "Patterns: 2 (target <= 6, compacted 0), History: 3 (target <= 6, compacted 1)"
        )
    );
    assert!(out.contains("archived 3, compacted 4, remaining 5"));
    // Lossless: a run that archived items points at the restore path.
    assert!(out.contains("restore with: ctx_knowledge action=restore"));
}

#[test]
fn consolidate_dry_run_previews_without_mutating() {
    let _env_lock = crate::core::data_dir::test_env_lock();
    let data_dir = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(data_dir.path());
    let project = tempfile::tempdir().unwrap();
    let root = project.path().to_string_lossy().to_string();
    let mut knowledge = ProjectKnowledge::new(&root);
    for i in 0..100 {
        knowledge
            .history
            .push(crate::core::knowledge::ConsolidatedInsight {
                summary: format!("summary {i}"),
                from_sessions: vec![format!("s{i}")],
                timestamp: Utc::now(),
            });
    }
    knowledge.save().unwrap();

    let report =
        consolidate_project_knowledge_with(&root, &ConsolidateOptions::manual().into_dry_run())
            .unwrap();

    // The preview reports the reclaim that *would* happen…
    assert!(report.dry_run);
    assert_eq!(report.history, 100);
    assert_eq!(report.history_compacted, 25);
    assert!(format_consolidation_report(&report).contains("DRY RUN"));

    // …but the store on disk is byte-for-byte untouched.
    let reloaded = ProjectKnowledge::load(&root).unwrap();
    assert_eq!(reloaded.history.len(), 100);
}

#[test]
fn consolidation_does_not_capacity_compact_at_twenty_five_percent_free() {
    let _env_lock = crate::core::data_dir::test_env_lock();
    let data_dir = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(data_dir.path());
    let project = tempfile::tempdir().unwrap();
    let root = project.path().to_string_lossy().to_string();
    let policy = MemoryPolicy::default();
    let mut knowledge = ProjectKnowledge::new(&root);

    for i in 0..150 {
        knowledge.remember(
            &format!("category-{i}"),
            &format!("k{i}"),
            &format!("unique stable fact value {i}"),
            "s1",
            0.8,
            &policy,
        );
    }
    for i in 0..75 {
        knowledge
            .history
            .push(crate::core::knowledge::ConsolidatedInsight {
                summary: format!("summary {i}"),
                from_sessions: vec![format!("s{i}")],
                timestamp: Utc::now(),
            });
    }
    knowledge.save().unwrap();

    let report = consolidate_project_knowledge(&root).unwrap();
    let reloaded = ProjectKnowledge::load(&root).unwrap();

    assert_eq!(report.fact_capacity_archived, 0);
    assert_eq!(report.history_compacted, 0);
    assert_eq!(reloaded.facts.len(), 150);
    assert_eq!(reloaded.history.len(), 75);
}

#[test]
fn consolidation_loads_session_for_requested_project_root() {
    let _env_lock = crate::core::data_dir::test_env_lock();
    let data_dir = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(data_dir.path());
    let cwd_project = tempfile::tempdir().unwrap();
    let target_project = tempfile::tempdir().unwrap();
    let cwd_root = cwd_project.path().to_string_lossy().to_string();
    let target_root = target_project.path().to_string_lossy().to_string();

    let mut cwd_session = SessionState::new();
    cwd_session.project_root = Some(cwd_root);
    cwd_session.add_finding(None, None, "wrong cwd finding");
    cwd_session.save().unwrap();

    let mut target_session = SessionState::new();
    target_session.project_root = Some(target_root.clone());
    target_session.add_finding(None, None, "target project finding");
    target_session.save().unwrap();

    let _cwd = CurrentDirGuard::enter(cwd_project.path());
    let report = consolidate_project_knowledge(&target_root).unwrap();

    assert_eq!(
        report.session_id.as_deref(),
        Some(target_session.id.as_str())
    );
    assert_eq!(report.session_items, 1);

    let knowledge = ProjectKnowledge::load(&target_root).unwrap();
    assert!(
        knowledge
            .facts
            .iter()
            .any(|f| f.value == "target project finding")
    );
    assert!(
        !knowledge
            .facts
            .iter()
            .any(|f| f.value == "wrong cwd finding")
    );
}
