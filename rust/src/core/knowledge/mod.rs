pub mod chunking;
mod core;
mod fact;
mod format;
pub mod maintenance;
mod persist;
mod query;
mod ranking;
mod types;

pub(crate) use ranking::sort_fact_for_output;
pub use ranking::{SimilarFact, find_cross_key_similar};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::memory_policy::MemoryPolicy;
    use chrono::Utc;

    fn default_policy() -> MemoryPolicy {
        MemoryPolicy::default()
    }

    fn current_count(k: &ProjectKnowledge) -> usize {
        k.facts.iter().filter(|f| f.is_current()).count()
    }

    // === Write-time admission ===

    #[test]
    fn admission_merges_same_category_near_duplicate_without_growing() {
        let policy = default_policy(); // enabled, merge 0.9, floor 0
        let mut k = ProjectKnowledge::new("/tmp/test-admission-merge");

        let r1 = k.remember_admitted(
            "architecture",
            "db-primary",
            "use postgres for the primary datastore",
            "s1",
            0.8,
            &policy,
        );
        assert!(matches!(r1, AdmissionResult::Stored(_)));
        assert_eq!(current_count(&k), 1);

        // Identical word set under a *different* key → merge, not insert.
        let r2 = k.remember_admitted(
            "architecture",
            "db-store",
            "use postgres for the primary datastore",
            "s2",
            0.9,
            &policy,
        );
        match r2 {
            AdmissionResult::Merged {
                category,
                key,
                confirmations,
                ..
            } => {
                assert_eq!(category, "architecture");
                assert_eq!(key, "db-primary", "merges into the earliest match");
                assert_eq!(confirmations, 2);
            }
            other @ AdmissionResult::Stored(_) => panic!("expected Merged, got {other:?}"),
        }
        assert_eq!(current_count(&k), 1, "merge must not grow the store");
        let f = k
            .facts
            .iter()
            .find(|f| f.key == "db-primary" && f.is_current())
            .unwrap();
        assert_eq!(f.confirmation_count, 2);
    }

    #[test]
    fn admission_preserves_contradiction_supersede_on_same_key() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test-admission-contradiction");

        let r1 = k.remember_admitted("arch", "db", "PostgreSQL", "s1", 0.95, &policy);
        assert!(matches!(r1, AdmissionResult::Stored(None)));

        // Same (category,key), conflicting value → must supersede, never merge/reject.
        let r2 = k.remember_admitted("arch", "db", "MySQL", "s2", 0.9, &policy);
        assert!(
            matches!(r2, AdmissionResult::Stored(Some(_))),
            "same-key conflict must surface a contradiction, got {r2:?}"
        );
        let cur = k
            .facts
            .iter()
            .find(|f| f.category == "arch" && f.key == "db" && f.is_current())
            .unwrap();
        assert_eq!(cur.value, "MySQL");
    }

    #[test]
    fn admission_does_not_merge_across_categories() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test-admission-xcat");

        k.remember_admitted(
            "architecture",
            "db",
            "use postgres for the primary datastore",
            "s1",
            0.8,
            &policy,
        );
        // Same value, *different category* → categories carry meaning, so no merge.
        let r = k.remember_admitted(
            "decision",
            "db",
            "use postgres for the primary datastore",
            "s2",
            0.9,
            &policy,
        );
        assert!(matches!(r, AdmissionResult::Stored(_)));
        assert_eq!(current_count(&k), 2);
    }

    #[test]
    fn remember_and_recall() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test-project");
        k.remember(
            "architecture",
            "auth",
            "JWT RS256",
            "session-1",
            0.9,
            &policy,
        );
        k.remember("api", "rate-limit", "100/min", "session-1", 0.8, &policy);

        let results = k.recall("auth");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "JWT RS256");

        let results = k.recall("api rate");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "rate-limit");
    }

    #[test]
    fn remember_infers_archetype_from_category() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test-archetype");
        k.remember("architecture", "db", "PostgreSQL", "s1", 0.9, &policy);
        k.remember("decision", "lang", "Rust", "s1", 0.9, &policy);
        k.remember("known_bugs", "src/a.rs:1", "panics", "s1", 0.9, &policy);
        k.remember("data_model", "user", "id,email", "s1", 0.9, &policy);
        k.remember("misc", "note", "whatever", "s1", 0.9, &policy);

        let arch = |cat: &str| {
            k.facts
                .iter()
                .find(|f| f.category == cat && f.is_current())
                .map(|f| f.archetype.clone())
                .expect("fact present")
        };
        assert_eq!(arch("architecture"), KnowledgeArchetype::Architecture);
        assert_eq!(arch("decision"), KnowledgeArchetype::Decision);
        assert_eq!(arch("known_bugs"), KnowledgeArchetype::Gotcha);
        assert_eq!(arch("data_model"), KnowledgeArchetype::Architecture);
        assert_eq!(arch("misc"), KnowledgeArchetype::Fact);
    }

    #[test]
    fn facts_evict_down_to_cap_not_double() {
        // remember() keeps the fact count at or below max_facts: a store never
        // grows past its configured budget before reclaiming.
        let mut policy = default_policy();
        policy.knowledge.max_facts = 5;
        let mut k = ProjectKnowledge::new("/tmp/test-evict");
        for i in 0..40 {
            k.remember(
                "finding",
                &format!("key-{i}"),
                &format!("value number {i}"),
                "s1",
                0.7,
                &policy,
            );
        }
        assert!(
            k.facts.len() <= policy.knowledge.max_facts,
            "expected <= {} facts after eviction, got {}",
            policy.knowledge.max_facts,
            k.facts.len()
        );
    }

    #[test]
    fn upsert_existing_fact() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.7, &policy);
        k.remember(
            "arch",
            "db",
            "PostgreSQL 16 with pgvector",
            "s2",
            0.95,
            &policy,
        );

        let current: Vec<_> = k.facts.iter().filter(|f| f.is_current()).collect();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].value, "PostgreSQL 16 with pgvector");
    }

    #[test]
    fn contradiction_detection() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.95, &policy);
        k.facts[0].confirmation_count = 3;

        let contradiction = k.check_contradiction("arch", "db", "MySQL", &policy);
        assert!(contradiction.is_some());
        let c = contradiction.unwrap();
        assert_eq!(c.severity, ContradictionSeverity::High);
    }

    #[test]
    fn temporal_validity() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.95, &policy);
        k.facts[0].confirmation_count = 3;

        k.remember("arch", "db", "MySQL", "s2", 0.9, &policy);

        let current: Vec<_> = k.facts.iter().filter(|f| f.is_current()).collect();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].value, "MySQL");

        let all_db: Vec<_> = k.facts.iter().filter(|f| f.key == "db").collect();
        assert_eq!(all_db.len(), 2);
    }

    #[test]
    fn confirmation_count() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.9, &policy);
        assert_eq!(k.facts[0].confirmation_count, 1);

        k.remember("arch", "db", "PostgreSQL", "s2", 0.9, &policy);
        assert_eq!(k.facts[0].confirmation_count, 2);
    }

    #[test]
    fn remove_fact() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.9, &policy);
        assert!(k.remove_fact("arch", "db"));
        assert!(k.facts.is_empty());
        assert!(!k.remove_fact("arch", "db"));
    }

    #[test]
    fn list_rooms() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("architecture", "auth", "JWT", "s1", 0.9, &policy);
        k.remember("architecture", "db", "PG", "s1", 0.9, &policy);
        k.remember("deploy", "host", "AWS", "s1", 0.8, &policy);

        let rooms = k.list_rooms();
        assert_eq!(rooms.len(), 2);
    }

    #[test]
    fn consolidate_history() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.consolidate(
            "Migrated from REST to GraphQL",
            vec!["s1".into(), "s2".into()],
            &policy,
        )
        .expect("consolidation succeeds");
        assert_eq!(k.history.len(), 1);
        assert_eq!(k.history[0].from_sessions.len(), 2);
    }

    #[test]
    fn format_summary_output() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("architecture", "auth", "JWT RS256", "s1", 0.9, &policy);
        let summary = k.format_summary();
        assert!(summary.contains("PROJECT KNOWLEDGE:"));
        assert!(summary.contains("auth: JWT RS256"));
    }

    #[test]
    fn temporal_recall_at_time() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.95, &policy);
        k.facts[0].confirmation_count = 3;

        let before_change = Utc::now();
        std::thread::sleep(std::time::Duration::from_millis(10));

        k.remember("arch", "db", "MySQL", "s2", 0.9, &policy);

        let results = k.recall_at_time("db", before_change);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "PostgreSQL");

        let results_now = k.recall_at_time("db", Utc::now());
        assert_eq!(results_now.len(), 1);
        assert_eq!(results_now[0].value, "MySQL");
    }

    #[test]
    fn wakeup_format() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "auth", "JWT", "s1", 0.95, &policy);
        k.remember("arch", "db", "PG", "s1", 0.8, &policy);

        // Theta-gamma chunked format: same-category facts share one
        // `[topic]` header that amortizes the category prefix.
        let wakeup = k.format_wakeup();
        assert!(wakeup.contains("FACTS:"));
        assert!(wakeup.contains("[arch]"));
        assert!(wakeup.contains("auth=JWT"));
        assert!(wakeup.contains("db=PG"));
    }

    #[test]
    fn salience_prioritizes_decisions_over_findings_at_similar_confidence() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("finding", "f1", "some thing", "s1", 0.9, &policy);
        k.remember("decision", "d1", "important", "s1", 0.85, &policy);

        // Chunked format: the founding fact of the first chunk is the
        // most salient one, so the decision chunk must render first.
        let wakeup = k.format_wakeup();
        let d = wakeup.find("d1=important").expect("decision in wakeup");
        let f = wakeup.find("f1=some thing").expect("finding in wakeup");
        assert!(
            d < f,
            "expected decision before finding in wakeup: {wakeup}"
        );
    }

    #[test]
    fn low_confidence_contradiction() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.4, &policy);

        let c = k.check_contradiction("arch", "db", "MySQL", &policy);
        assert!(c.is_some());
        assert_eq!(c.unwrap().severity, ContradictionSeverity::Low);
    }

    #[test]
    fn no_contradiction_for_same_value() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.95, &policy);

        let c = k.check_contradiction("arch", "db", "PostgreSQL", &policy);
        assert!(c.is_none());
    }

    #[test]
    fn no_contradiction_for_similar_values() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember(
            "arch",
            "db",
            "PostgreSQL 16 production database server",
            "s1",
            0.95,
            &policy,
        );

        let c = k.check_contradiction(
            "arch",
            "db",
            "PostgreSQL 16 production database server config",
            &policy,
        );
        assert!(c.is_none());
    }

    #[test]
    fn revision_count_on_new_fact() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.9, &policy);
        let cur = k.facts.iter().find(|f| f.is_current()).unwrap();
        assert_eq!(cur.revision_count, 1);
    }

    #[test]
    fn revision_count_increments_on_confirm() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.9, &policy);
        k.remember("arch", "db", "PostgreSQL", "s2", 0.9, &policy);
        k.remember("arch", "db", "PostgreSQL", "s3", 0.9, &policy);
        let cur = k.facts.iter().find(|f| f.is_current()).unwrap();
        assert_eq!(cur.revision_count, 3);
        assert_eq!(cur.confirmation_count, 3);
    }

    #[test]
    fn revision_count_carries_over_on_supersede() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.95, &policy);
        k.remember("arch", "db", "PostgreSQL", "s2", 0.9, &policy);
        assert_eq!(
            k.facts
                .iter()
                .find(|f| f.is_current())
                .unwrap()
                .revision_count,
            2
        );
        k.facts[0].confirmation_count = 3;
        k.remember("arch", "db", "MySQL", "s3", 0.9, &policy);
        let cur: Vec<_> = k.facts.iter().filter(|f| f.is_current()).collect();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].value, "MySQL");
        assert_eq!(cur[0].revision_count, 3);
        assert!(cur[0].supersedes.is_some());
    }

    #[test]
    fn cross_key_similar_finds_related_facts() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember(
            "architecture",
            "auth",
            "JWT RS256 token based authentication with Redis session store",
            "s1",
            0.9,
            &policy,
        );
        k.remember(
            "decision",
            "session-model",
            "JWT token authentication stored in Redis for session management",
            "s1",
            0.85,
            &policy,
        );
        k.remember("deploy", "host", "AWS us-east-1", "s1", 0.8, &policy);

        let similar = find_cross_key_similar(
            "architecture",
            "auth",
            "JWT RS256 token based authentication with Redis session store",
            &k.facts,
            3,
        );
        assert!(!similar.is_empty(), "should find session-model as similar");
        assert_eq!(similar[0].category, "decision");
        assert_eq!(similar[0].key, "session-model");
        assert!(similar[0].similarity > 0.35);
    }

    /// The preview of a long similar value is cut on a character boundary.
    #[test]
    fn cross_key_similar_previews_multibyte_values() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        let value = "я".repeat(40);
        k.remember("arch", "a", &value, "s1", 0.9, &policy);
        let similar = find_cross_key_similar("arch", "b", &value, &k.facts, 3);
        assert_eq!(similar.len(), 1);
        assert!(similar[0].value_preview.ends_with("..."));
        assert!(value.starts_with(similar[0].value_preview.trim_end_matches("...")));
    }

    #[test]
    fn cross_key_similar_excludes_same_key() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember("arch", "db", "PostgreSQL 16", "s1", 0.9, &policy);

        let similar = find_cross_key_similar("arch", "db", "PostgreSQL 16", &k.facts, 3);
        assert!(similar.is_empty());
    }

    #[test]
    fn cross_key_similar_ignores_unrelated_facts() {
        let policy = default_policy();
        let mut k = ProjectKnowledge::new("/tmp/test");
        k.remember(
            "arch",
            "db",
            "PostgreSQL 16 with pgvector",
            "s1",
            0.9,
            &policy,
        );
        k.remember("deploy", "host", "AWS us-east-1 region", "s1", 0.8, &policy);

        let similar =
            find_cross_key_similar("arch", "db", "PostgreSQL 16 with pgvector", &k.facts, 3);
        assert!(similar.is_empty(), "unrelated facts should not match");
    }
}
