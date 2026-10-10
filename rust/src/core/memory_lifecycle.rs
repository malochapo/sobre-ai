//! Memory Lifecycle Management — consolidation, decay, compaction, archival.
//!
//! Runs automatically on knowledge stores to keep memory healthy:
//! - Confidence decay over time
//! - Semantic consolidation of similar facts
//! - Compaction when limits are exceeded
//! - Archival of old/unused facts

use crate::core::text_similarity::token_jaccard_f32;
use chrono::{Duration, Utc};
use std::path::PathBuf;

use super::knowledge::{KnowledgeFact, sort_fact_for_output};
use super::memory_archive::{ArchiveConfig, MemoryStore};

/// Default proactive headroom on a capacity reclaim: settle a full store at 75%
/// so it keeps real working room instead of churning at its cap.
pub const DEFAULT_RECLAIM_HEADROOM_PCT: f32 = 0.25;

/// Spacing/testing effect: how strongly each prior retrieval lengthens memory
/// stability. 0.5 ⇒ ~10 retrievals make a fact roughly 6× more durable.
const SPACING_GAIN: f32 = 0.5;
/// Floor on derived stability (days) so even a heavily down-voted fact decays
/// smoothly rather than collapsing in a single pass.
const MIN_STABILITY_DAYS: f32 = 1.0;
/// Confidence never decays below this — archival happens elsewhere, decay never
/// hard-deletes.
const CONFIDENCE_FLOOR: f32 = 0.05;
/// Default characteristic memory stability (days) for the Ebbinghaus curve.
pub const DEFAULT_BASE_STABILITY_DAYS: f32 = 90.0;

#[derive(Debug, Clone)]
pub struct LifecycleConfig {
    pub max_facts: usize,
    pub low_confidence_threshold: f32,
    pub stale_days: i64,
    pub consolidation_similarity: f32,
    /// Characteristic stability (days) for the Ebbinghaus curve before spacing
    /// and feedback modulation.
    pub base_stability_days: f32,
    /// Archive facts untouched for this many days that were **never** retrieved —
    /// dead weight that costs injection tokens regardless of confidence.
    /// `None` disables it. Reversible: pruned facts go to the archive.
    pub prune_unretrieved_after_days: Option<i64>,
    /// Proactive headroom on a capacity reclaim: when a store reaches its
    /// cap, drop down to `reclaim_target(max_facts, reclaim_headroom_pct)` so it
    /// settles with working room instead of churning at the cap. `0.25` = 75%.
    pub reclaim_headroom_pct: f32,
}

/// The production policy's values (`MemoryPolicy::default()`), so tests run
/// against the same numbers as the server.
impl Default for LifecycleConfig {
    fn default() -> Self {
        Self::from_policy(&crate::core::memory_policy::MemoryPolicy::default())
    }
}

impl LifecycleConfig {
    /// Map the persisted [`crate::core::memory_policy::MemoryPolicy`] to the
    /// runtime lifecycle config. The single mapping site, so adding a knob
    /// touches exactly one place.
    pub fn from_policy(policy: &crate::core::memory_policy::MemoryPolicy) -> Self {
        Self {
            max_facts: policy.knowledge.max_facts,
            low_confidence_threshold: policy.lifecycle.low_confidence_threshold,
            stale_days: policy.lifecycle.stale_days,
            consolidation_similarity: policy.lifecycle.similarity_threshold,
            base_stability_days: policy.lifecycle.base_stability_days,
            prune_unretrieved_after_days: policy.lifecycle.prune_unretrieved_after_days,
            reclaim_headroom_pct: policy.lifecycle.reclaim_headroom_pct,
        }
    }
}

#[derive(Debug, Default)]
pub struct LifecycleReport {
    pub decayed_count: usize,
    pub consolidated_count: usize,
    pub archived_count: usize,
    pub compacted_count: usize,
    /// Of `archived_count`, how many facts were evicted purely for capacity
    /// (the proactive reclaim) versus quality (low-confidence/stale/unretrieved).
    /// Lets callers report per-store capacity reclaim distinctly.
    pub capacity_archived: usize,
    pub remaining_facts: usize,
}

pub fn apply_confidence_decay(facts: &mut [KnowledgeFact], config: &LifecycleConfig) -> usize {
    let now = Utc::now();
    let mut count = 0;

    for fact in facts.iter_mut() {
        if !fact.is_current() {
            continue;
        }

        if let Some(valid_until) = fact.valid_until
            && valid_until < now
            && fact.confidence > 0.1
        {
            fact.confidence = 0.1;
            count += 1;
            continue;
        }

        let days_since_confirmed = now.signed_duration_since(fact.last_confirmed).num_days() as f32;
        if days_since_confirmed <= 0.0 {
            continue;
        }
        let days_since_retrieved = fact
            .last_retrieved
            .map_or(3650.0, |t| now.signed_duration_since(t).num_days() as f32);
        let retrieval_count = fact.retrieval_count as f32;
        let net_feedback = i64::from(fact.feedback_up) - i64::from(fact.feedback_down);

        let new_confidence = ebbinghaus_confidence(
            fact.confidence,
            days_since_confirmed,
            days_since_retrieved,
            retrieval_count,
            net_feedback,
            config.base_stability_days,
        );
        if (new_confidence - fact.confidence).abs() > 0.001 {
            fact.confidence = new_confidence;
            count += 1;
        }
    }

    count
}

/// Ebbinghaus retention `R = exp(-Δt / S)`. Stability `S` grows with the
/// spacing effect (each prior retrieval) and net feedback; `Δt` is time since
/// the memory was last reinforced (confirmed *or* retrieved). Multiplicative so
/// confidence approaches the floor smoothly and never overshoots. Deterministic.
fn ebbinghaus_confidence(
    confidence: f32,
    days_since_confirmed: f32,
    days_since_retrieved: f32,
    retrieval_count: f32,
    net_feedback: i64,
    base_stability_days: f32,
) -> f32 {
    let elapsed = days_since_confirmed.min(days_since_retrieved).max(0.0);
    let spacing = 1.0 + SPACING_GAIN * retrieval_count;
    let feedback_mult = match net_feedback.cmp(&0) {
        std::cmp::Ordering::Greater => 1.0 + (net_feedback as f32).ln_1p(),
        std::cmp::Ordering::Less => 1.0 / (1.0 + (net_feedback.unsigned_abs() as f32).ln_1p()),
        std::cmp::Ordering::Equal => 1.0,
    };
    let stability = (base_stability_days * spacing * feedback_mult).max(MIN_STABILITY_DAYS);
    let retention = (-(f64::from(elapsed)) / f64::from(stability)).exp() as f32;
    (confidence * retention).max(CONFIDENCE_FLOOR)
}

pub fn consolidate_similar(facts: &mut Vec<KnowledgeFact>, similarity_threshold: f32) -> usize {
    let mut to_remove: std::collections::HashSet<usize> = std::collections::HashSet::new();

    let mut category_groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, f) in facts.iter().enumerate() {
        if f.is_current() {
            category_groups
                .entry(f.category.clone())
                .or_default()
                .push(i);
        }
    }

    for indices in category_groups.values() {
        for (pos_a, &i) in indices.iter().enumerate() {
            if to_remove.contains(&i) {
                continue;
            }
            for &j in &indices[pos_a + 1..] {
                if to_remove.contains(&j) {
                    continue;
                }
                let sim = token_jaccard_f32(&facts[i].value, &facts[j].value);
                if sim >= similarity_threshold {
                    if facts[i].confidence >= facts[j].confidence {
                        facts[i].confirmation_count += facts[j].confirmation_count;
                        if facts[j].last_confirmed > facts[i].last_confirmed {
                            facts[i].last_confirmed = facts[j].last_confirmed;
                        }
                        to_remove.insert(j);
                    } else {
                        facts[j].confirmation_count += facts[i].confirmation_count;
                        if facts[i].last_confirmed > facts[j].last_confirmed {
                            facts[j].last_confirmed = facts[i].last_confirmed;
                        }
                        to_remove.insert(i);
                        break;
                    }
                }
            }
        }
    }

    let count = to_remove.len();
    let mut sorted: Vec<usize> = to_remove.into_iter().collect();
    sorted.sort_unstable();
    for idx in sorted.into_iter().rev() {
        facts.remove(idx);
    }

    count
}

pub fn compact(
    facts: &mut Vec<KnowledgeFact>,
    config: &LifecycleConfig,
) -> (usize, Vec<KnowledgeFact>) {
    let mut archived: Vec<KnowledgeFact> = Vec::new();
    let now = Utc::now();
    let stale_threshold = now - Duration::days(config.stale_days);

    let mut to_archive: Vec<usize> = Vec::new();

    for (i, fact) in facts.iter().enumerate() {
        let recently_retrieved = fact
            .last_retrieved
            .is_some_and(|t| now.signed_duration_since(t).num_days() < 14);
        let frequently_retrieved = fact.retrieval_count >= 5;

        if fact.confidence < config.low_confidence_threshold {
            to_archive.push(i);
            continue;
        }

        // Real pruning: a single-confirmation fact untouched for the
        // configured horizon that was *never* retrieved is dead weight even at
        // high confidence — archive it. Gated on `confirmation_count <= 1` so
        // repeatedly-confirmed (structurally important) facts are always kept.
        if let Some(days) = config.prune_unretrieved_after_days {
            let cutoff = now - Duration::days(days);
            if fact.last_confirmed < cutoff
                && fact.retrieval_count == 0
                && fact.last_retrieved.is_none()
                && fact.confirmation_count <= 1
            {
                to_archive.push(i);
                continue;
            }
        }

        if fact.last_confirmed < stale_threshold
            && fact.confirmation_count <= 1
            && fact.confidence < 0.5
            && !recently_retrieved
            && !frequently_retrieved
        {
            to_archive.push(i);
        }
    }

    to_archive.sort_unstable();
    to_archive.dedup();

    for idx in to_archive.into_iter().rev() {
        archived.push(facts.remove(idx));
    }

    // Quality-only archival here. Capacity reclaim moved to `run_lifecycle` so it
    // flows through the single capacity manager ([`crate::core::memory_capacity`])
    // like every other store, keeping `compact` a pure quality pass.
    (archived.len(), archived)
}

/// Runs decay, consolidation, quality archival and the capacity reclaim over
/// the facts of the project whose hash is `scope`. Archived facts go to that
/// project's archive. When an archive cannot be written, the facts it would
/// have held stay in `facts` and the error is returned.
pub fn run_lifecycle(
    facts: &mut Vec<KnowledgeFact>,
    scope: &str,
    config: &LifecycleConfig,
) -> Result<LifecycleReport, String> {
    let decayed = apply_confidence_decay(facts, config);
    let consolidated = consolidate_similar(facts, config.consolidation_similarity);
    let before_compaction = facts.clone();
    let (compacted, archived) = compact(facts, config);

    if let Err(e) = archive_facts(scope, &archived) {
        *facts = before_compaction;
        return Err(e);
    }

    // Capacity reclaim: facts settle at headroom via the single capacity
    // manager, archiving the evicted tail in the same project archive quality
    // archival uses, so recall rehydration is uniform.
    let capacity_archived = crate::core::memory_capacity::reclaim_store(
        MemoryStore::Facts,
        scope,
        facts,
        config.max_facts,
        config.reclaim_headroom_pct,
        |a, b| {
            b.is_current()
                .cmp(&a.is_current())
                .then_with(|| sort_fact_for_output(a, b))
        },
    )?
    .len();

    Ok(LifecycleReport {
        decayed_count: decayed,
        consolidated_count: consolidated,
        archived_count: archived.len() + capacity_archived,
        compacted_count: compacted + capacity_archived,
        capacity_archived,
        remaining_facts: facts.len(),
    })
}

/// Archive evicted facts in the archive of the project whose hash is `scope`;
/// the generic multi-store archive lives in [`crate::core::memory_archive`].
pub fn archive_facts(scope: &str, facts: &[KnowledgeFact]) -> Result<(), String> {
    crate::core::memory_archive::archive_items(
        MemoryStore::Facts,
        scope,
        facts,
        &ArchiveConfig::default(),
    )
    .map(|_| ())
}

/// Restore the facts from a single archive file.
pub fn restore_archive(archive_path: &str) -> Result<Vec<KnowledgeFact>, String> {
    crate::core::memory_archive::restore_items(std::path::Path::new(archive_path))
}

/// The newest facts archives of the project whose hash is `scope`, for the
/// recall-miss rehydrate path — bounded by [`ArchiveConfig::rehydrate_reach`]
/// so every retained archive is reachable.
pub fn reachable_archives(scope: &str, cfg: &ArchiveConfig) -> Vec<PathBuf> {
    crate::core::memory_archive::reachable_archives(MemoryStore::Facts, scope, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::knowledge::KnowledgeArchetype;

    /// Capacity reclaim archives the evicted tail to disk, so any test that drives
    /// [`run_lifecycle`] over capacity must sandbox the data dir.
    fn with_temp_data_dir<T>(f: impl FnOnce() -> T) -> T {
        let _lock = crate::core::data_dir::test_env_lock();
        let dir = std::env::temp_dir().join(format!(
            "sobre-lifecycle-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        crate::test_env::set_var("SOBRE_DATA_DIR", dir.to_str().unwrap());
        let out = f();
        crate::test_env::remove_var("SOBRE_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn make_fact(category: &str, key: &str, value: &str, confidence: f32) -> KnowledgeFact {
        KnowledgeFact {
            category: category.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            source_session: "s1".to_string(),
            confidence,
            created_at: Utc::now(),
            last_confirmed: Utc::now(),
            retrieval_count: 0,
            last_retrieved: None,
            valid_from: Some(Utc::now()),
            valid_until: None,
            supersedes: None,
            confirmation_count: 1,
            feedback_up: 0,
            feedback_down: 0,
            last_feedback: None,
            sensitivity: crate::core::sensitivity::SensitivityLevel::default(),
            imported_from: None,
            archetype: KnowledgeArchetype::default(),
            revision_count: 0,
        }
    }

    fn make_old_fact(
        category: &str,
        key: &str,
        value: &str,
        confidence: f32,
        days_old: i64,
    ) -> KnowledgeFact {
        let past = Utc::now() - Duration::days(days_old);
        KnowledgeFact {
            category: category.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            source_session: "s1".to_string(),
            confidence,
            created_at: past,
            last_confirmed: past,
            retrieval_count: 0,
            last_retrieved: None,
            valid_from: Some(past),
            valid_until: None,
            supersedes: None,
            confirmation_count: 1,
            feedback_up: 0,
            feedback_down: 0,
            last_feedback: None,
            sensitivity: crate::core::sensitivity::SensitivityLevel::default(),
            imported_from: None,
            archetype: KnowledgeArchetype::default(),
            revision_count: 0,
        }
    }

    #[test]
    fn decay_reduces_confidence() {
        let config = LifecycleConfig::default();
        let mut facts = vec![make_old_fact("arch", "db", "PostgreSQL", 0.9, 10)];

        let count = apply_confidence_decay(&mut facts, &config);
        assert_eq!(count, 1);
        assert!(facts[0].confidence < 0.9);
        assert!(facts[0].confidence > 0.7);
    }

    #[test]
    fn decay_skips_recent_facts() {
        let config = LifecycleConfig::default();
        let mut facts = vec![make_fact("arch", "db", "PostgreSQL", 0.9)];

        let count = apply_confidence_decay(&mut facts, &config);
        assert_eq!(count, 0);
    }

    #[test]
    fn feedback_steers_decay_keep_vs_forget() {
        let config = LifecycleConfig::default();
        let mut praised = make_old_fact("arch", "loved", "keep me", 0.9, 10);
        praised.feedback_up = 5;
        let mut panned = make_old_fact("arch", "hated", "forget me", 0.9, 10);
        panned.feedback_down = 5;
        let neutral = make_old_fact("arch", "meh", "neutral", 0.9, 10);

        let mut facts = vec![praised, panned, neutral];
        apply_confidence_decay(&mut facts, &config);

        let (praised_c, panned_c, neutral_c) = (
            facts[0].confidence,
            facts[1].confidence,
            facts[2].confidence,
        );

        // Reward bridge: up-voted retains more than neutral, neutral more than down-voted.
        assert!(
            praised_c > neutral_c,
            "praised {praised_c} should outlast neutral {neutral_c}"
        );
        assert!(
            neutral_c > panned_c,
            "neutral {neutral_c} should outlast panned {panned_c}"
        );
        // Even a heavily down-voted fact only fades toward the floor — never hard-deleted.
        assert!(panned_c >= 0.05);
    }

    #[test]
    fn spacing_effect_protects_frequently_retrieved() {
        // Under the Ebbinghaus curve, a fact retrieved many times must decay
        // slower than an identical never-retrieved fact of the same age.
        let config = LifecycleConfig::default();
        let rarely = make_old_fact("arch", "rare", "x", 0.9, 20);
        let mut often = make_old_fact("arch", "often", "y", 0.9, 20);
        often.retrieval_count = 20;
        let mut facts = vec![rarely, often];
        apply_confidence_decay(&mut facts, &config);
        assert!(
            facts[1].confidence > facts[0].confidence,
            "spacing effect: rehearsed {} should outlast un-rehearsed {}",
            facts[1].confidence,
            facts[0].confidence
        );
    }

    #[test]
    fn ebbinghaus_decay_is_deterministic() {
        // Determinism contract: same input → same output, no RNG.
        let config = LifecycleConfig::default();
        let mut a = vec![make_old_fact("arch", "k", "v", 0.8, 15)];
        let mut b = a.clone();
        apply_confidence_decay(&mut a, &config);
        apply_confidence_decay(&mut b, &config);
        assert_eq!(a[0].confidence, b[0].confidence);
    }

    #[test]
    fn consolidate_similar_facts() {
        let mut facts = vec![
            make_fact("arch", "db", "uses PostgreSQL database", 0.8),
            make_fact("arch", "db2", "uses PostgreSQL database system", 0.6),
            make_fact("ops", "deploy", "docker compose up", 0.9),
        ];

        let count = consolidate_similar(&mut facts, 0.7);
        assert!(count > 0, "Should consolidate similar facts");
        assert!(facts.len() < 3);
    }

    #[test]
    fn consolidate_keeps_different_categories() {
        let mut facts = vec![
            make_fact("arch", "db", "PostgreSQL", 0.8),
            make_fact("ops", "db", "PostgreSQL", 0.8),
        ];

        let count = consolidate_similar(&mut facts, 0.9);
        assert_eq!(count, 0, "Different categories should not consolidate");
    }

    #[test]
    fn compact_removes_low_confidence() {
        let config = LifecycleConfig::default();
        let mut facts = vec![
            make_fact("arch", "db", "PostgreSQL", 0.9),
            make_fact("arch", "cache", "Redis", 0.1),
        ];

        let (count, archived) = compact(&mut facts, &config);
        assert_eq!(count, 1);
        assert_eq!(facts.len(), 1);
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].key, "cache");
    }

    #[test]
    fn compact_is_quality_only_and_ignores_capacity() {
        // Capacity reclaim moved out of `compact` into the single
        // capacity manager (driven by `run_lifecycle`). A store full of healthy,
        // current, high-confidence facts is a *capacity* concern, so `compact`
        // (quality only) must leave it untouched.
        let config = LifecycleConfig {
            max_facts: 8,
            ..Default::default()
        };
        let mut facts: Vec<KnowledgeFact> = (0..8)
            .map(|i| make_fact("finding", &format!("k{i}"), &format!("value {i}"), 0.8))
            .collect();

        let (count, archived) = compact(&mut facts, &config);

        assert_eq!(count, 0, "quality compact must not evict for capacity");
        assert!(archived.is_empty());
        assert_eq!(facts.len(), 8);
    }

    #[test]
    fn run_lifecycle_reclaims_capacity_to_headroom() {
        with_temp_data_dir(|| {
            let config = LifecycleConfig {
                max_facts: 8,
                ..Default::default()
            };
            let mut facts: Vec<KnowledgeFact> = (0..8)
                .map(|i| make_fact("finding", &format!("k{i}"), &format!("value {i}"), 0.8))
                .collect();

            let report = run_lifecycle(&mut facts, "scope", &config).expect("lifecycle succeeds");

            // Hysteresis: at cap (8) → settle to headroom target (6), archive 2.
            assert_eq!(report.capacity_archived, 2);
            assert_eq!(facts.len(), 6);
            assert_eq!(report.remaining_facts, 6);
        });
    }

    #[test]
    fn run_lifecycle_evicts_expired_before_current() {
        with_temp_data_dir(|| {
            let config = LifecycleConfig {
                max_facts: 4,
                ..Default::default()
            };
            let decision = make_fact("decision", "keep-decision", "important decision", 0.7);
            let finding = make_fact("finding", "keep-finding", "fresh finding", 0.9);
            let mut old = make_fact("decision", "drop-archived", "old decision", 0.95);
            // Definitively expired (not just "now") so retention ordering is stable.
            old.valid_until = Some(Utc::now() - Duration::seconds(1));
            let low = make_fact("misc", "drop-low", "low salience", 0.6);
            let mut facts = vec![old, low, finding, decision];

            let report = run_lifecycle(&mut facts, "scope", &config).expect("lifecycle succeeds");
            let keys: Vec<&str> = facts.iter().map(|f| f.key.as_str()).collect();

            // 4 at cap → settle to 3; the expired fact sorts last (not current) and
            // is the single eviction, so every current fact survives.
            assert_eq!(report.capacity_archived, 1);
            assert!(keys.contains(&"keep-decision"));
            assert!(keys.contains(&"keep-finding"));
            assert!(!keys.contains(&"drop-archived"));
        });
    }

    /// A fact archived in one project is never brought back into another.
    #[test]
    fn archived_facts_stay_in_their_project() {
        with_temp_data_dir(|| {
            let policy = crate::core::memory_policy::MemoryPolicy::default();
            let mut a = crate::core::knowledge::ProjectKnowledge::new("/tmp/sobre-archive-proj-a");
            a.remember("architecture", "db", "PostgreSQL", "s1", 0.1, &policy);
            a.run_memory_lifecycle(&policy).expect("lifecycle succeeds");
            assert!(a.facts.is_empty(), "the low-confidence fact is archived");

            let mut b = crate::core::knowledge::ProjectKnowledge::new("/tmp/sobre-archive-proj-b");
            let found = crate::tools::sobre_knowledge::rehydrate_from_archives(
                &mut b,
                None,
                Some("postgresql"),
                "s2",
                &policy,
            );
            assert!(!found, "project B must not see project A's archive");
            assert!(b.facts.is_empty());

            let mut a_again =
                crate::core::knowledge::ProjectKnowledge::new("/tmp/sobre-archive-proj-a");
            assert!(crate::tools::sobre_knowledge::rehydrate_from_archives(
                &mut a_again,
                None,
                Some("postgresql"),
                "s3",
                &policy,
            ));
        });
    }

    /// When the archive cannot be written, the facts it would have held stay.
    #[test]
    fn a_failed_archive_keeps_the_facts() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        // A file where the data directory should be: no archive can be created.
        let not_a_dir = tmp.path().join("data");
        std::fs::write(&not_a_dir, "").unwrap();
        crate::test_env::set_var("SOBRE_DATA_DIR", &not_a_dir);

        let mut facts = vec![
            make_fact("arch", "db", "PostgreSQL", 0.9),
            make_fact("arch", "cache", "Redis", 0.1),
        ];
        let result = run_lifecycle(&mut facts, "scope", &LifecycleConfig::default());
        crate::test_env::remove_var("SOBRE_DATA_DIR");

        assert!(result.is_err());
        let keys: Vec<&str> = facts.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["db", "cache"]);
    }

    #[test]
    fn prune_unretrieved_archives_old_never_retrieved_facts() {
        // Opt-in: a 60-day-old, high-confidence, never-retrieved,
        // single-confirmation fact is dead weight and must be archived even
        // though its confidence is well above the low-confidence floor.
        let config = LifecycleConfig {
            prune_unretrieved_after_days: Some(30),
            ..Default::default()
        };
        let mut facts = vec![make_old_fact("arch", "x", "still confident", 0.9, 60)];
        let (count, archived) = compact(&mut facts, &config);
        assert_eq!(count, 1);
        assert_eq!(archived.len(), 1);
        assert!(facts.is_empty());
    }

    #[test]
    fn prune_unretrieved_spares_a_fact_younger_than_the_default_90_days() {
        // The default prunes a never-retrieved fact after 90 days, so a
        // confident 60-day-old one is kept.
        let config = LifecycleConfig::default();
        assert_eq!(config.prune_unretrieved_after_days, Some(90));
        let mut facts = vec![make_old_fact("arch", "x", "still confident", 0.9, 60)];
        let (count, _) = compact(&mut facts, &config);
        assert_eq!(count, 0);
        assert_eq!(facts.len(), 1);
    }

    #[test]
    fn prune_unretrieved_keeps_retrieved_and_confirmed_facts() {
        let config = LifecycleConfig {
            prune_unretrieved_after_days: Some(30),
            ..Default::default()
        };
        let mut retrieved = make_old_fact("arch", "used", "v", 0.9, 60);
        retrieved.retrieval_count = 3;
        let mut confirmed = make_old_fact("arch", "confirmed", "v", 0.9, 60);
        confirmed.confirmation_count = 4;
        let mut facts = vec![retrieved, confirmed];
        let (count, _) = compact(&mut facts, &config);
        assert_eq!(count, 0, "retrieved or repeatedly-confirmed facts are kept");
        assert_eq!(facts.len(), 2);
    }

    #[test]
    fn compact_archives_stale_facts() {
        let config = LifecycleConfig::default();
        let mut facts = vec![
            make_fact("arch", "db", "PostgreSQL", 0.9),
            make_old_fact("arch", "old", "ancient thing", 0.4, 60),
        ];

        let (count, archived) = compact(&mut facts, &config);
        assert_eq!(count, 1);
        assert_eq!(archived[0].key, "old");
    }

    #[test]
    fn full_lifecycle_run() {
        // Compaction archives the stale facts to disk.
        with_temp_data_dir(|| {
            let config = LifecycleConfig {
                max_facts: 5,
                ..Default::default()
            };

            let mut facts = vec![
                make_fact("arch", "db", "PostgreSQL", 0.9),
                make_fact("arch", "cache", "Redis", 0.8),
                make_old_fact("arch", "old1", "thing1", 0.2, 50),
                make_old_fact("arch", "old2", "thing2", 0.15, 60),
                make_fact("ops", "deploy", "docker compose", 0.7),
            ];

            let report = run_lifecycle(&mut facts, "scope", &config).expect("lifecycle succeeds");
            assert!(report.remaining_facts <= config.max_facts);
            assert!(report.decayed_count > 0 || report.compacted_count > 0);
        });
    }
}
