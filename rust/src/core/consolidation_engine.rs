//! Session→knowledge consolidation: the one session-import primitive
//! (`import_session_into`) and its options.
//!
//! The full, locked orchestrator (import + history + lifecycle + per-store
//! reclaim + report) lives in
//! `sobre_knowledge::consolidate_project_knowledge_with`.

use crate::core::knowledge::ProjectKnowledge;
use crate::core::memory_policy::MemoryPolicy;
use crate::core::session::{Finding, SessionState};

/// Confidence assigned to imported decisions.
const DECISION_CONFIDENCE: f32 = 0.85;
/// Confidence assigned to imported findings.
const FINDING_CONFIDENCE: f32 = 0.7;

/// How a consolidation run behaves: a real run, or a preview.
#[derive(Debug, Clone, Default)]
pub struct ConsolidateOptions {
    /// Compute the report without mutating knowledge, archives or the session.
    pub dry_run: bool,
}

impl ConsolidateOptions {
    /// Explicit CLI / MCP `consolidate`: import everything, full lifecycle and a
    /// lossless reclaim of every store.
    pub fn manual() -> Self {
        Self::default()
    }

    /// Same plan, but preview-only: no writes to knowledge, archives or session.
    #[must_use]
    pub fn into_dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }
}

/// Counts of items promoted by a single `import_session_into` call.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImportCounts {
    pub decisions: usize,
    pub findings: usize,
}

impl ImportCounts {
    pub fn total(self) -> usize {
        self.decisions + self.findings
    }
}

/// The single session→knowledge import. Operates on an already-locked
/// `knowledge` (no I/O, no lock), so the locked orchestrator and the dry-run
/// preview share one implementation.
pub(crate) fn import_session_into(
    knowledge: &mut ProjectKnowledge,
    session: &SessionState,
    policy: &MemoryPolicy,
) -> ImportCounts {
    let mut decisions: Vec<&crate::core::session::Decision> = session.decisions.iter().collect();
    decisions.sort_by_key(|d| std::cmp::Reverse(d.timestamp));
    for d in &decisions {
        let key = slug_key(&d.summary, 50);
        knowledge.remember(
            "decision",
            &key,
            &d.summary,
            &session.id,
            DECISION_CONFIDENCE,
            policy,
        );
    }

    let mut findings: Vec<&Finding> = session.findings.iter().collect();
    findings.sort_by_key(|f| std::cmp::Reverse(f.timestamp));
    for f in &findings {
        let key = finding_key(f);
        knowledge.remember(
            "finding",
            &key,
            &f.summary,
            &session.id,
            FINDING_CONFIDENCE,
            policy,
        );
    }

    ImportCounts {
        decisions: decisions.len(),
        findings: findings.len(),
    }
}

/// Stable knowledge key for a session finding: `file[:line]` when located, else a
/// content slug. Content-based (never index-based), so re-imports upsert the same
/// fact and the output stays deterministic across runs.
pub(crate) fn finding_key(f: &Finding) -> String {
    match (&f.file, f.line) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        (Some(file), None) => file.clone(),
        (None, _) => format!("finding-{}", slug_key(&f.summary, 36)),
    }
}

/// Deterministic, filesystem-safe slug for a fact key: lowercase alphanumerics,
/// single dashes for separators, trimmed, capped at `max` bytes.
pub(crate) fn slug_key(s: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if out.len() >= max {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch.is_whitespace() || ch == '-' || ch == '_')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn consolidate_promotes_decisions_and_findings() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        crate::test_env::set_var("SOBRE_DATA_DIR", tmp.path().to_string_lossy().to_string());

        let project_root = tmp.path().join("proj");
        std::fs::create_dir_all(&project_root).expect("mkdir");
        let project_root_str = project_root.to_string_lossy().to_string();

        let mut session = SessionState::new();
        session.project_root = Some(project_root_str.clone());
        session.add_decision("Use archive-only memory lifecycle", None);
        session.add_finding(None, None, "panic: index out of bounds");
        session.add_finding(None, None, "just a note");
        session.save().expect("save session");

        let out = crate::tools::sobre_knowledge::consolidate_project_knowledge_with(
            &project_root_str,
            &ConsolidateOptions::manual(),
        )
        .expect("consolidate");
        assert_eq!(out.session_items, 3, "one decision and two findings");

        let k = ProjectKnowledge::load(&project_root_str).expect("knowledge saved");
        let active = k.facts.iter().filter(|f| f.is_current()).count();
        assert!(active >= 2, "expected promoted facts");

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn finding_key_is_content_based_and_deterministic() {
        let f1 = Finding {
            file: Some("src/main.rs".into()),
            line: Some(42),
            summary: "boom".into(),
            timestamp: Utc::now(),
        };
        assert_eq!(finding_key(&f1), "src/main.rs:42");

        let f2 = Finding {
            file: None,
            line: None,
            summary: "Race condition in cache".into(),
            timestamp: Utc::now(),
        };
        // Same content → same key (idempotent re-import, no index drift).
        assert_eq!(finding_key(&f2), finding_key(&f2));
        assert_eq!(finding_key(&f2), "finding-race-condition-in-cache");
    }
}
