//! Built-in memory limits. Not configurable: every caller uses `MemoryPolicy::default()`.

#[derive(Debug, Clone, Default)]
pub struct MemoryPolicy {
    pub knowledge: KnowledgePolicy,
    pub lifecycle: LifecyclePolicy,
    pub embeddings: EmbeddingsPolicy,
    pub admission: AdmissionPolicy,
}

#[derive(Debug, Clone)]
pub struct KnowledgePolicy {
    pub max_facts: usize,
    pub max_patterns: usize,
    pub max_history: usize,
    pub contradiction_threshold: f32,
    /// Maximum number of facts returned by recall operations.
    pub recall_facts_limit: usize,
    /// Maximum number of rooms returned by `sobre_knowledge action=rooms`.
    pub rooms_limit: usize,
    /// Maximum number of timeline entries returned by `sobre_knowledge action=timeline`.
    pub timeline_limit: usize,
    /// Maximum number of relations/edges returned by relations queries/diagrams.
    pub relations_limit: usize,
}

impl Default for KnowledgePolicy {
    fn default() -> Self {
        Self {
            max_facts: 200,
            max_patterns: 50,
            max_history: 100,
            contradiction_threshold: 0.5,
            recall_facts_limit: crate::core::budgets::KNOWLEDGE_RECALL_FACTS_LIMIT,
            rooms_limit: crate::core::budgets::KNOWLEDGE_ROOMS_LIMIT,
            timeline_limit: crate::core::budgets::KNOWLEDGE_TIMELINE_LIMIT,
            relations_limit: 40,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LifecyclePolicy {
    pub low_confidence_threshold: f32,
    pub stale_days: i64,
    pub similarity_threshold: f32,
    /// Characteristic memory stability in days for the Ebbinghaus curve.
    pub base_stability_days: f32,
    /// Archive single-confirmation facts untouched for this many days that were
    /// never retrieved — dead weight regardless of confidence. Defaults to
    /// a conservative 90 days: genuinely cold facts are archived (and
    /// rehydrate on recall), so a store self-curates instead of only churning at
    /// its cap.
    pub prune_unretrieved_after_days: Option<i64>,
    /// Proactive headroom on a capacity reclaim: when a store reaches its
    /// cap, settle it at `1 - reclaim_headroom_pct` (e.g. `0.25` → 75%) instead
    /// of churning right at the cap. Lossless — the reclaimed tail is archived
    /// and restorable.
    pub reclaim_headroom_pct: f32,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            low_confidence_threshold: 0.3,
            stale_days: 30,
            similarity_threshold: 0.85,
            base_stability_days: crate::core::memory_lifecycle::DEFAULT_BASE_STABILITY_DAYS,
            prune_unretrieved_after_days: Some(90),
            reclaim_headroom_pct: crate::core::memory_lifecycle::DEFAULT_RECLAIM_HEADROOM_PCT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddingsPolicy {
    pub max_facts: usize,
}

impl Default for EmbeddingsPolicy {
    fn default() -> Self {
        Self { max_facts: 2000 }
    }
}

/// Write-time admission control for the knowledge store. The cap +
/// importance-eviction is a backstop *after* the fact is written; admission is
/// the boundary *before* it, so a capped store fills with signal, not noise.
/// Applied only to direct `sobre_knowledge remember` (the agent-facing path);
/// internal restorers (archive rehydrate) bypass it.
#[derive(Debug, Clone)]
pub struct AdmissionPolicy {
    /// A new fact whose value is at least this similar (word-Jaccard, 0.0–1.0) to
    /// an existing *current* fact in the **same category** under a different key
    /// is merged into it (a confirmation bump) instead of inserted as a new row.
    /// High so only genuine near-duplicates collapse.
    pub auto_merge_similarity: f32,
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        Self {
            auto_merge_similarity: 0.9,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_discipline_defaults_are_premium() {
        // Self-curation is on by default, but lossless.
        let p = MemoryPolicy::default();
        assert_eq!(
            p.lifecycle.prune_unretrieved_after_days,
            Some(90),
            "conservative recoverable prune default"
        );
    }
}
