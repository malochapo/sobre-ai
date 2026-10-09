//! Consolidation engine: session import, fact lifecycle, lossless capacity
//! reclaim and the consolidation reports.

use crate::core::consolidation_engine::{ConsolidateOptions, ImportCounts, import_session_into};
use crate::core::knowledge::ProjectKnowledge;
use crate::core::memory_archive::MemoryStore;
use crate::core::memory_capacity::{reclaim_preview, reclaim_store, reclaim_target};
use crate::core::memory_lifecycle::LifecycleReport;
use crate::core::memory_policy::MemoryPolicy;
use crate::core::session::SessionState;

#[derive(Debug, Default)]
pub(crate) struct KnowledgeConsolidationReport {
    pub session_id: Option<String>,
    pub session_items: usize,
    pub facts: usize,
    pub active_facts: usize,
    pub archived_facts: usize,
    pub fact_capacity_target: usize,
    pub fact_capacity_archived: usize,
    pub patterns: usize,
    pub patterns_capacity_target: usize,
    pub patterns_compacted: usize,
    pub history: usize,
    pub history_capacity_target: usize,
    pub history_compacted: usize,
    pub lifecycle: LifecycleReport,
    /// True when produced by a preview run (no knowledge/archive/session writes).
    pub dry_run: bool,
}

/// Explicit CLI / MCP `consolidate`: import the whole session, run the fact
/// lifecycle and losslessly reclaim every store. Thin wrapper over the canonical
/// [`consolidate_project_knowledge_with`].
pub(crate) fn consolidate_project_knowledge(
    project_root: &str,
) -> Result<KnowledgeConsolidationReport, String> {
    consolidate_project_knowledge_with(project_root, &ConsolidateOptions::manual())
}

/// Consolidation: session import, fact lifecycle and the lossless per-store
/// capacity reclaim. Session loads are project-scoped (not cwd-scoped), and
/// `opts.dry_run` previews without mutating knowledge, archives or the session.
pub(crate) fn consolidate_project_knowledge_with(
    project_root: &str,
    opts: &ConsolidateOptions,
) -> Result<KnowledgeConsolidationReport, String> {
    let policy = crate::core::memory_policy::MemoryPolicy::default();
    let session = SessionState::load_latest_for_project_root(project_root);

    if opts.dry_run {
        return Ok(dry_run_report(project_root, session.as_ref(), &policy));
    }

    let (_knowledge, report) = ProjectKnowledge::mutate_locked(project_root, |knowledge| {
        run_consolidation_locked(knowledge, session.as_ref(), &policy)
    })
    .map_err(|e| format!("Consolidation done but save failed: {e}"))?;
    report
}

/// The locked read-modify-write body of a real consolidation run.
fn run_consolidation_locked(
    knowledge: &mut ProjectKnowledge,
    session: Option<&SessionState>,
    policy: &MemoryPolicy,
) -> Result<KnowledgeConsolidationReport, String> {
    let mut imported = ImportCounts::default();
    let mut session_id = None;
    let mut history_compacted = 0usize;

    if let Some(s) = session {
        session_id = Some(s.id.clone());
        imported = import_session_into(knowledge, s, policy);

        let task_desc = s
            .task
            .as_ref()
            .map_or_else(|| "(no task)".into(), |t| t.description.clone());
        let summary = format!(
            "Session {}: {} — {} findings, {} decisions consolidated",
            s.id, task_desc, imported.findings, imported.decisions
        );
        // `consolidate` records the insight and losslessly reclaims history.
        history_compacted += knowledge.consolidate(&summary, vec![s.id.clone()], policy)?;
    }

    let lifecycle = knowledge.run_memory_lifecycle(policy)?;

    // Lossless capacity reclaim for the non-fact stores (facts settle inside the
    // lifecycle). History is already bounded per consolidate; the explicit pass
    // also compacts a pre-existing over-cap history when no session was imported.
    let patterns_compacted = reclaim_patterns(knowledge, policy)?;
    history_compacted += reclaim_history(knowledge, policy)?;
    let active_facts = knowledge.facts.iter().filter(|f| f.is_current()).count();
    let archived_facts = knowledge.facts.len().saturating_sub(active_facts);
    let headroom = policy.lifecycle.reclaim_headroom_pct;

    Ok(KnowledgeConsolidationReport {
        session_id,
        session_items: imported.total(),
        facts: knowledge.facts.len(),
        active_facts,
        archived_facts,
        fact_capacity_target: reclaim_target(policy.knowledge.max_facts, headroom),
        fact_capacity_archived: lifecycle.capacity_archived,
        patterns: knowledge.patterns.len(),
        patterns_capacity_target: reclaim_target(policy.knowledge.max_patterns, headroom),
        patterns_compacted,
        history: knowledge.history.len(),
        history_capacity_target: reclaim_target(policy.knowledge.max_history, headroom),
        history_compacted,
        lifecycle,
        dry_run: false,
    })
}

/// Preview a consolidation on a throwaway clone: identical math, zero writes to
/// knowledge, archives or the session. Reuses the real lifecycle/import code so
/// the counts match what a non-dry run would produce (Phase 6).
fn dry_run_report(
    project_root: &str,
    session: Option<&SessionState>,
    policy: &MemoryPolicy,
) -> KnowledgeConsolidationReport {
    let mut knowledge =
        ProjectKnowledge::load(project_root).unwrap_or_else(|| ProjectKnowledge::new(project_root));
    let headroom = policy.lifecycle.reclaim_headroom_pct;

    let mut imported = ImportCounts::default();
    let mut session_id = None;
    if let Some(s) = session {
        session_id = Some(s.id.clone());
        // remember() is in-memory only, so importing into the clone is side
        // effect free; it gives the exact promotion counts.
        imported = import_session_into(&mut knowledge, s, policy);
    }

    // Fact lifecycle preview: run the pure in-memory passes (no archive writes),
    // then preview the capacity reclaim.
    let lifecycle = {
        let cfg = crate::core::memory_lifecycle::LifecycleConfig::from_policy(policy);
        let decayed =
            crate::core::memory_lifecycle::apply_confidence_decay(&mut knowledge.facts, &cfg);
        let consolidated = crate::core::memory_lifecycle::consolidate_similar(
            &mut knowledge.facts,
            cfg.consolidation_similarity,
        );
        let (quality, _) = crate::core::memory_lifecycle::compact(&mut knowledge.facts, &cfg);
        let capacity_archived = reclaim_preview(knowledge.facts.len(), cfg.max_facts, headroom);
        LifecycleReport {
            decayed_count: decayed,
            consolidated_count: consolidated,
            archived_count: quality + capacity_archived,
            compacted_count: quality + capacity_archived,
            capacity_archived,
            remaining_facts: knowledge.facts.len().saturating_sub(capacity_archived),
        }
    };

    let history_compacted = reclaim_preview(
        knowledge.history.len(),
        policy.knowledge.max_history,
        headroom,
    );
    let patterns_compacted = reclaim_preview(
        knowledge.patterns.len(),
        policy.knowledge.max_patterns,
        headroom,
    );
    let active_facts = knowledge.facts.iter().filter(|f| f.is_current()).count();
    let archived_facts = knowledge.facts.len().saturating_sub(active_facts);

    KnowledgeConsolidationReport {
        session_id,
        session_items: imported.total(),
        facts: knowledge.facts.len(),
        active_facts,
        archived_facts,
        fact_capacity_target: reclaim_target(policy.knowledge.max_facts, headroom),
        fact_capacity_archived: lifecycle.capacity_archived,
        patterns: knowledge.patterns.len(),
        patterns_capacity_target: reclaim_target(policy.knowledge.max_patterns, headroom),
        patterns_compacted,
        history: knowledge.history.len(),
        history_capacity_target: reclaim_target(policy.knowledge.max_history, headroom),
        history_compacted,
        lifecycle,
        dry_run: true,
    }
}

/// Lossless history capacity reclaim. Returns the number of insights archived.
fn reclaim_history(
    knowledge: &mut ProjectKnowledge,
    policy: &MemoryPolicy,
) -> Result<usize, String> {
    reclaim_store(
        MemoryStore::History,
        Some(&knowledge.project_hash),
        &mut knowledge.history,
        policy.knowledge.max_history,
        policy.lifecycle.reclaim_headroom_pct,
        |a, b| {
            b.timestamp
                .cmp(&a.timestamp)
                .then_with(|| b.summary.cmp(&a.summary))
        },
    )
    .map(|archived| archived.len())
}

/// Lossless pattern capacity reclaim (newest kept). Returns the archived count.
fn reclaim_patterns(
    knowledge: &mut ProjectKnowledge,
    policy: &MemoryPolicy,
) -> Result<usize, String> {
    reclaim_store(
        MemoryStore::Patterns,
        Some(&knowledge.project_hash),
        &mut knowledge.patterns,
        policy.knowledge.max_patterns,
        policy.lifecycle.reclaim_headroom_pct,
        |a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.pattern_type.cmp(&b.pattern_type))
                .then_with(|| a.description.cmp(&b.description))
        },
    )
    .map(|archived| archived.len())
}

pub(crate) fn format_consolidation_report(report: &KnowledgeConsolidationReport) -> String {
    let session_line = match report.session_id.as_deref() {
        Some(session_id) => {
            format!(
                "Session import: {session_id} ({} item(s))",
                report.session_items
            )
        }
        None => "Session import: none (no active session)".to_string(),
    };

    let banner = if report.dry_run {
        "DRY RUN — preview only, no changes written\n"
    } else {
        ""
    };

    let body = format!(
        "{banner}{session_line}\n\
         Facts: {} active, {} archived, {} total (target <= {}, archived-to-target {})\n\
         Patterns: {} (target <= {}, compacted {}), History: {} (target <= {}, compacted {})\n\
         Lifecycle: decayed {}, consolidated {}, archived {}, compacted {}, remaining {}",
        report.active_facts,
        report.archived_facts,
        report.facts,
        report.fact_capacity_target,
        report.fact_capacity_archived,
        report.patterns,
        report.patterns_capacity_target,
        report.patterns_compacted,
        report.history,
        report.history_capacity_target,
        report.history_compacted,
        report.lifecycle.decayed_count,
        report.lifecycle.consolidated_count,
        report.lifecycle.archived_count,
        report.lifecycle.compacted_count,
        report.lifecycle.remaining_facts
    );

    // Eviction is lossless: if anything was (or would be) archived this run, point
    // the user at the explicit restore path.
    let archived_total =
        report.fact_capacity_archived + report.patterns_compacted + report.history_compacted;
    if archived_total > 0 {
        let verb = if report.dry_run {
            "would archive"
        } else {
            "archived"
        };
        format!(
            "{body}\n{verb} {archived_total} item(s) — restore with: sobre_knowledge action=restore"
        )
    } else {
        body
    }
}
