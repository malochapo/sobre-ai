//! Tests for the sobre_knowledge action handlers and consolidation reports.

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::core::consolidation_engine::ConsolidateOptions;
use crate::core::memory_lifecycle::LifecycleReport;
use chrono::Utc;

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
        facts: 7,
        active_facts: 5,
        archived_facts: 2,
        fact_capacity_target: 6,
        fact_capacity_archived: 1,
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
    assert!(out.contains("History: 3 (target <= 6, compacted 1)"));
    assert!(out.contains("archived 3, compacted 4, remaining 5"));
    // Lossless: a run that archived items says so.
    assert!(out.contains("archived 2 item(s)"));
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

    let report = consolidate_project_knowledge_with(
        &root,
        None,
        &ConsolidateOptions::manual().into_dry_run(),
    )
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

    let report =
        consolidate_project_knowledge_with(&root, None, &ConsolidateOptions::manual()).unwrap();
    let reloaded = ProjectKnowledge::load(&root).unwrap();

    assert_eq!(report.fact_capacity_archived, 0);
    assert_eq!(report.history_compacted, 0);
    assert_eq!(reloaded.facts.len(), 150);
    assert_eq!(reloaded.history.len(), 75);
}

/// `consolidate` imports the session it is given (the live one, unsaved
/// findings included), and a second run confirms nothing twice.
#[test]
fn consolidating_the_live_session_twice_confirms_each_fact_once() {
    let _env_lock = crate::core::data_dir::test_env_lock();
    let data_dir = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(data_dir.path());
    let project = tempfile::tempdir().unwrap();
    let root = project.path().to_string_lossy().to_string();

    let mut live = SessionState::new();
    live.project_root = Some(root.clone());
    live.add_finding(None, None, "unsaved finding from this conversation");

    let opts = ConsolidateOptions::manual();
    let first = consolidate_project_knowledge_with(&root, Some(&live), &opts).unwrap();
    assert_eq!(first.session_items, 1);
    let second = consolidate_project_knowledge_with(&root, Some(&live), &opts).unwrap();
    assert_eq!(second.session_items, 0, "already imported");

    let knowledge = ProjectKnowledge::load(&root).unwrap();
    let fact = knowledge
        .facts
        .iter()
        .find(|f| f.value == "unsaved finding from this conversation")
        .expect("imported");
    assert_eq!(fact.confirmation_count, 1);
}

#[test]
fn search_reads_only_the_requested_project() {
    let _env_lock = crate::core::data_dir::test_env_lock();
    let data_dir = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(data_dir.path());
    let other_project = tempfile::tempdir().unwrap();
    let target_project = tempfile::tempdir().unwrap();
    let other_root = other_project.path().to_string_lossy().to_string();
    let target_root = target_project.path().to_string_lossy().to_string();
    let policy = MemoryPolicy::default();

    for (root, tag) in [(&target_root, "target"), (&other_root, "other")] {
        let mut knowledge = ProjectKnowledge::new(root);
        knowledge.remember(
            "deploy",
            "pipeline",
            &format!("retries happen in the {tag} pipeline"),
            "s1",
            0.8,
            &policy,
        );
        knowledge.save().unwrap();

        let mut session = SessionState::new();
        session.project_root = Some(root.clone());
        session.add_finding(None, None, &format!("{tag} finding about retries"));
        session.add_decision(&format!("{tag} decision about retries"), None);
        session.save().unwrap();
    }

    // The server's cwd is another project: the search must follow the
    // session's project root, not the process directory.
    let _cwd = CurrentDirGuard::enter(other_project.path());
    let out = handle_search(&target_root, Some("retries"));

    assert!(
        out.contains("retries happen in the target pipeline"),
        "{out}"
    );
    assert!(out.contains("target finding about retries"), "{out}");
    assert!(out.contains("target decision about retries"), "{out}");
    assert!(!out.contains("other"), "another project leaked in: {out}");
}

/// A superseded fact is not a search result.
#[test]
fn search_leaves_superseded_facts_out() {
    let _env_lock = crate::core::data_dir::test_env_lock();
    let data_dir = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(data_dir.path());
    let project = tempfile::tempdir().unwrap();
    let root = project.path().to_string_lossy().to_string();
    let policy = MemoryPolicy::default();

    let mut knowledge = ProjectKnowledge::new(&root);
    knowledge.remember(
        "deploy",
        "region",
        "deploys go to eu-west-1",
        "s1",
        0.8,
        &policy,
    );
    knowledge.remember(
        "deploy",
        "region",
        "deploys go to us-east-2",
        "s2",
        0.8,
        &policy,
    );
    knowledge.save().unwrap();

    let out = handle_search(&root, Some("deploys"));
    assert!(out.contains("us-east-2"), "{out}");
    assert!(
        !out.contains("eu-west-1"),
        "a superseded fact came back: {out}"
    );
}

/// A fact that could not be written is reported as an error, never as
/// remembered or removed.
#[test]
fn a_failed_save_is_an_error() {
    let _dir = crate::core::data_dir::isolated_data_dir();
    let blocker = tempfile::NamedTempFile::new().unwrap();
    // A data dir that is a file: nothing can be written under it.
    crate::test_env::set_var("SOBRE_DATA_DIR", blocker.path());
    let project = tempfile::tempdir().unwrap();
    let root = project.path().to_string_lossy().to_string();

    let out = handle_remember(
        &root,
        Some("deploy"),
        Some("region"),
        Some("eu-west-1"),
        "s1",
        None,
    );
    assert!(out.starts_with("ERROR"), "{out}");
    assert!(!out.contains("Remembered"), "{out}");
}

/// Removing from a project with no knowledge finds nothing and writes nothing.
#[test]
fn remove_without_knowledge_writes_nothing() {
    let _dir = crate::core::data_dir::isolated_data_dir();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().to_string_lossy().to_string();

    let out = handle_remove(&root, Some("deploy"), Some("region"));
    assert!(out.starts_with("No fact found"), "{out}");
    assert!(
        ProjectKnowledge::load(&root).is_none(),
        "a store was created"
    );
}
