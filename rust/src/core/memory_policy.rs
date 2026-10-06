use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MemoryPolicy {
    pub knowledge: KnowledgePolicy,
    pub episodic: EpisodicPolicy,
    pub procedural: ProceduralPolicy,
    pub lifecycle: LifecyclePolicy,
    pub embeddings: EmbeddingsPolicy,
    pub gotcha: GotchaPolicy,
    pub admission: AdmissionPolicy,
    pub compaction: CompactionPolicy,
}

impl MemoryPolicy {
    pub fn validate(&self) -> Result<(), String> {
        self.knowledge.validate()?;
        self.episodic.validate()?;
        self.procedural.validate()?;
        self.lifecycle.validate()?;
        self.embeddings.validate()?;
        self.gotcha.validate()?;
        self.admission.validate()?;
        self.compaction.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgePolicy {
    pub max_facts: usize,
    pub max_patterns: usize,
    pub max_history: usize,
    pub contradiction_threshold: f32,
    /// Maximum number of facts returned by recall operations.
    pub recall_facts_limit: usize,
    /// Maximum number of rooms returned by `ctx_knowledge action=rooms`.
    pub rooms_limit: usize,
    /// Maximum number of timeline entries returned by `ctx_knowledge action=timeline`.
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

impl KnowledgePolicy {
    fn validate(&self) -> Result<(), String> {
        if self.max_facts == 0 {
            return Err("memory.knowledge.max_facts must be > 0".to_string());
        }
        if self.max_patterns == 0 {
            return Err("memory.knowledge.max_patterns must be > 0".to_string());
        }
        if self.max_history == 0 {
            return Err("memory.knowledge.max_history must be > 0".to_string());
        }
        if !(0.0..=1.0).contains(&self.contradiction_threshold) {
            return Err(
                "memory.knowledge.contradiction_threshold must be in [0.0, 1.0]".to_string(),
            );
        }
        if self.recall_facts_limit == 0 {
            return Err("memory.knowledge.recall_facts_limit must be > 0".to_string());
        }
        if self.rooms_limit == 0 {
            return Err("memory.knowledge.rooms_limit must be > 0".to_string());
        }
        if self.timeline_limit == 0 {
            return Err("memory.knowledge.timeline_limit must be > 0".to_string());
        }
        if self.relations_limit == 0 {
            return Err("memory.knowledge.relations_limit must be > 0".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EpisodicPolicy {
    pub max_episodes: usize,
    pub max_actions_per_episode: usize,
    pub summary_max_chars: usize,
}

impl Default for EpisodicPolicy {
    fn default() -> Self {
        Self {
            max_episodes: 500,
            max_actions_per_episode: 50,
            summary_max_chars: 200,
        }
    }
}

impl EpisodicPolicy {
    fn validate(&self) -> Result<(), String> {
        if self.max_episodes == 0 {
            return Err("memory.episodic.max_episodes must be > 0".to_string());
        }
        if self.max_actions_per_episode == 0 {
            return Err("memory.episodic.max_actions_per_episode must be > 0".to_string());
        }
        if self.summary_max_chars < 40 {
            return Err("memory.episodic.summary_max_chars must be >= 40".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProceduralPolicy {
    pub min_repetitions: usize,
    pub min_sequence_len: usize,
    pub max_procedures: usize,
    pub max_window_size: usize,
}

impl Default for ProceduralPolicy {
    fn default() -> Self {
        Self {
            min_repetitions: 3,
            min_sequence_len: 2,
            max_procedures: 100,
            max_window_size: 10,
        }
    }
}

impl ProceduralPolicy {
    fn validate(&self) -> Result<(), String> {
        if self.min_repetitions == 0 {
            return Err("memory.procedural.min_repetitions must be > 0".to_string());
        }
        if self.min_sequence_len < 2 {
            return Err("memory.procedural.min_sequence_len must be >= 2".to_string());
        }
        if self.max_procedures == 0 {
            return Err("memory.procedural.max_procedures must be > 0".to_string());
        }
        if self.max_window_size < self.min_sequence_len {
            return Err(
                "memory.procedural.max_window_size must be >= min_sequence_len".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LifecyclePolicy {
    pub decay_rate: f32,
    pub low_confidence_threshold: f32,
    pub stale_days: i64,
    pub similarity_threshold: f32,
    /// Forgetting curve (#1): `ebbinghaus` (default) or `linear` (legacy).
    pub forgetting_model: String,
    /// Characteristic memory stability in days for the Ebbinghaus curve.
    pub base_stability_days: f32,
    /// Scale Ebbinghaus stability by fact archetype so structural evidence decays
    /// slower than inference. Default false keeps the baseline tuning unchanged.
    pub archetype_aware_decay: bool,
    /// Archive single-confirmation facts untouched for this many days that were
    /// never retrieved — dead weight regardless of confidence (#962). Defaults to
    /// a conservative 90 days (#972): genuinely cold facts are archived (and
    /// rehydrate on recall), so a store self-curates instead of only churning at
    /// its cap. Set `None`/`off` to disable.
    pub prune_unretrieved_after_days: Option<i64>,
    /// Proactive headroom on a capacity reclaim (#995): when a store reaches its
    /// cap, settle it at `1 - reclaim_headroom_pct` (e.g. `0.25` → 75%) instead
    /// of churning right at the cap. Lossless — the reclaimed tail is archived
    /// and restorable.
    pub reclaim_headroom_pct: f32,
    /// Master switch for the proactive reclaim (#995). `false` is the documented
    /// escape hatch: trim only the overflow, no headroom. Eviction stays lossless
    /// either way.
    pub reclaim_enabled: bool,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            decay_rate: 0.01,
            low_confidence_threshold: 0.3,
            stale_days: 30,
            similarity_threshold: 0.85,
            forgetting_model: "ebbinghaus".to_string(),
            base_stability_days: crate::core::memory_lifecycle::DEFAULT_BASE_STABILITY_DAYS,
            archetype_aware_decay: false,
            prune_unretrieved_after_days: Some(90),
            reclaim_headroom_pct: crate::core::memory_lifecycle::DEFAULT_RECLAIM_HEADROOM_PCT,
            reclaim_enabled: true,
        }
    }
}

impl LifecyclePolicy {
    fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.decay_rate) {
            return Err("memory.lifecycle.decay_rate must be in [0.0, 1.0]".to_string());
        }
        if !(0.0..=1.0).contains(&self.low_confidence_threshold) {
            return Err(
                "memory.lifecycle.low_confidence_threshold must be in [0.0, 1.0]".to_string(),
            );
        }
        if self.stale_days < 0 {
            return Err("memory.lifecycle.stale_days must be >= 0".to_string());
        }
        if !(0.0..=1.0).contains(&self.similarity_threshold) {
            return Err("memory.lifecycle.similarity_threshold must be in [0.0, 1.0]".to_string());
        }
        if self.base_stability_days <= 0.0 {
            return Err("memory.lifecycle.base_stability_days must be > 0".to_string());
        }
        if !(0.0..=0.95).contains(&self.reclaim_headroom_pct) {
            return Err("memory.lifecycle.reclaim_headroom_pct must be in [0.0, 0.95]".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingsPolicy {
    pub max_facts: usize,
}

impl Default for EmbeddingsPolicy {
    fn default() -> Self {
        Self { max_facts: 2000 }
    }
}

impl EmbeddingsPolicy {
    fn validate(&self) -> Result<(), String> {
        if self.max_facts == 0 {
            return Err("memory.embeddings.max_facts must be > 0".to_string());
        }
        Ok(())
    }
}

use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GotchaPolicy {
    pub max_gotchas_per_project: usize,
    pub retrieval_budget_per_room: usize,
    pub default_decay_rate: f32,
    pub category_decay_overrides: HashMap<String, f32>,
}

impl Default for GotchaPolicy {
    fn default() -> Self {
        Self {
            max_gotchas_per_project: 100,
            retrieval_budget_per_room: 10,
            default_decay_rate: 0.03,
            category_decay_overrides: HashMap::new(),
        }
    }
}

impl GotchaPolicy {
    fn validate(&self) -> Result<(), String> {
        if self.max_gotchas_per_project == 0 {
            return Err("memory.gotcha.max_gotchas_per_project must be > 0".to_string());
        }
        if self.retrieval_budget_per_room == 0 {
            return Err("memory.gotcha.retrieval_budget_per_room must be > 0".to_string());
        }
        if !(0.0..=1.0).contains(&self.default_decay_rate) {
            return Err("memory.gotcha.default_decay_rate must be 0.0-1.0".to_string());
        }
        Ok(())
    }
}

/// Write-time admission control for the knowledge store (#970). The cap +
/// importance-eviction is a backstop *after* the fact is written; admission is
/// the boundary *before* it, so a capped store fills with signal, not noise.
/// Applied only to direct `ctx_knowledge remember` (the agent-facing path);
/// internal restorers (archive rehydrate, cognition auto-promotion) bypass it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdmissionPolicy {
    /// Master switch for write-time admission. When off, every `remember`
    /// inserts as before (legacy behavior).
    pub enabled: bool,
    /// A new fact whose value is at least this similar (word-Jaccard, 0.0–1.0) to
    /// an existing *current* fact in the **same category** under a different key
    /// is merged into it (a confirmation bump) instead of inserted as a new row.
    /// High by default so only genuine near-duplicates collapse; `0.0` disables
    /// auto-merge.
    pub auto_merge_similarity: f32,
    /// Facts whose content salience ([`crate::core::memory_salience::text_salience`])
    /// is below this floor are not admitted as normal facts. `0` (default)
    /// disables the floor — the lossless choice; raise it to curate a noisy store.
    pub min_salience: u32,
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_merge_similarity: 0.9,
            min_salience: 0,
        }
    }
}

impl AdmissionPolicy {
    fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.auto_merge_similarity) {
            return Err("memory.admission.auto_merge_similarity must be in [0.0, 1.0]".to_string());
        }
        Ok(())
    }
}

/// Cluster compaction (#971): the background cognition loop collapses piles of
/// low-value, mutually-similar facts into a single recoverable digest, so a busy
/// store's live fact count actually *drops* instead of churning at its cap. It is
/// strictly guarded — only faded (`< max_confidence`), barely-confirmed
/// (`<= max_confirmations`), never-frequently/recently-retrieved facts in a
/// cluster of at least `min_cluster` qualify — and lossless, since the originals
/// are archived and rehydrate on recall.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionPolicy {
    /// Master switch for cluster compaction in the cognition loop.
    pub enabled: bool,
    /// Minimum number of facts in a same-category cluster before it is collapsed
    /// into a digest. Must be `>= 2`.
    pub min_cluster: usize,
    /// Average word-Jaccard similarity (0.0–1.0) a fact needs to join a cluster.
    pub similarity: f32,
    /// Importance ceiling: only facts *below* this confidence are eligible, so a
    /// high-confidence fact is never compacted.
    pub max_confidence: f32,
    /// Only facts confirmed at most this many times are eligible — a
    /// repeatedly-confirmed fact is structurally important and always kept.
    pub max_confirmations: u32,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            min_cluster: 4,
            similarity: 0.5,
            max_confidence: 0.5,
            max_confirmations: 1,
        }
    }
}

impl CompactionPolicy {
    fn validate(&self) -> Result<(), String> {
        if self.min_cluster < 2 {
            return Err("memory.compaction.min_cluster must be >= 2".to_string());
        }
        if !(0.0..=1.0).contains(&self.similarity) {
            return Err("memory.compaction.similarity must be in [0.0, 1.0]".to_string());
        }
        if !(0.0..=1.0).contains(&self.max_confidence) {
            return Err("memory.compaction.max_confidence must be in [0.0, 1.0]".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_valid() {
        let p = MemoryPolicy::default();
        p.validate().expect("default policy must be valid");
    }

    #[test]
    fn memory_discipline_defaults_are_premium() {
        // #970/#971/#972: self-curation is on by default, but lossless.
        let p = MemoryPolicy::default();
        assert!(p.admission.enabled, "admission on by default");
        assert_eq!(p.admission.min_salience, 0, "salience floor off (lossless)");
        assert!(p.compaction.enabled, "cluster compaction on by default");
        assert!(p.compaction.min_cluster >= 2);
        assert_eq!(
            p.lifecycle.prune_unretrieved_after_days,
            Some(90),
            "conservative recoverable prune default"
        );
    }

    #[test]
    fn validate_rejects_invalid_compaction() {
        let mut p = MemoryPolicy::default();
        p.compaction.min_cluster = 1;
        assert!(p.validate().is_err());

        let mut p = MemoryPolicy::default();
        p.admission.auto_merge_similarity = 1.5;
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_values() {
        let mut p = MemoryPolicy::default();
        p.knowledge.max_facts = 0;
        assert!(p.validate().is_err());

        let mut p = MemoryPolicy::default();
        p.lifecycle.decay_rate = 2.0;
        assert!(p.validate().is_err());

        let mut p = MemoryPolicy::default();
        p.procedural.min_sequence_len = 1;
        assert!(p.validate().is_err());
    }
}
