//! `KnowledgeSnapshot` — the single in-memory view of a project's durable
//! knowledge that every *outbound* rendering shares.
//!
//! Before this existed, the Context Package builder read `knowledge.json` +
//! `relations.json` inline and shaped its own `KnowledgeLayer`. Adding a second
//! portable format (Open Knowledge Format, see [`super::okf`]) would have meant a
//! *second* extractor reading the same stores with subtly different rules — the
//! exact fragmentation the "one model, many renderings" positioning warns
//! against. Instead both the ctxpkg `KnowledgeLayer` and the OKF Markdown bundle
//! are rendered from this one snapshot, so they can never drift on *what counts
//! as the project's knowledge*.
//!
//! The snapshot carries the full fact history (ctxpkg preserves superseded facts
//! for fidelity); [`KnowledgeSnapshot::current_facts`] is the human-facing view
//! portable exports use.

use crate::core::knowledge_relations::{KnowledgeEdge, KnowledgeRelationGraph};

use super::types::{ConsolidatedInsight, KnowledgeFact, ProjectKnowledge, ProjectPattern};

/// A consistent read of a project's knowledge (facts + patterns + insights) and
/// its relation graph, taken at one point in time. The single source of truth
/// for outbound renderings (ctxpkg, OKF).
#[derive(Debug, Clone)]
pub struct KnowledgeSnapshot {
    pub project_hash: String,
    /// All facts, including superseded ones (ctxpkg keeps the history).
    pub facts: Vec<KnowledgeFact>,
    pub patterns: Vec<ProjectPattern>,
    /// Consolidated insights (the project's `history`).
    pub insights: Vec<ConsolidatedInsight>,
    /// Typed relations between facts (`relations.json`).
    pub relations: Vec<KnowledgeEdge>,
}

impl KnowledgeSnapshot {
    /// Loads a project's knowledge and relation graph from disk into one
    /// snapshot. Missing stores yield empty collections rather than an error, so
    /// callers can treat "no knowledge yet" via [`KnowledgeSnapshot::is_empty`].
    pub fn collect(project_root: &str) -> Self {
        let knowledge = ProjectKnowledge::load_or_create(project_root);
        let relations = KnowledgeRelationGraph::load(&knowledge.project_hash)
            .map(|g| g.edges)
            .unwrap_or_default();
        Self::from_project(&knowledge, relations)
    }

    /// Builds a snapshot from an already-loaded `ProjectKnowledge` plus its
    /// relation edges. Keeps the collection logic in one place for callers that
    /// already hold the knowledge (e.g. inside a lock).
    pub fn from_project(knowledge: &ProjectKnowledge, relations: Vec<KnowledgeEdge>) -> Self {
        Self {
            project_hash: knowledge.project_hash.clone(),
            facts: knowledge.facts.clone(),
            patterns: knowledge.patterns.clone(),
            insights: knowledge.history.clone(),
            relations,
        }
    }

    /// True when there is nothing worth exporting (no facts, patterns, or
    /// insights). Relations alone never make a bundle — they are edges between
    /// facts that, without endpoints, carry no standalone meaning.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.patterns.is_empty() && self.insights.is_empty()
    }

    /// The current (non-superseded, temporally valid) facts — the human-facing
    /// view portable exports render. Superseded history stays in [`Self::facts`]
    /// for ctxpkg fidelity but would only confuse a hand-edited OKF bundle.
    pub fn current_facts(&self) -> Vec<&KnowledgeFact> {
        self.facts.iter().filter(|f| f.is_current()).collect()
    }
}
