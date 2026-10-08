//! Embedding-based Knowledge Retrieval for `sobre_knowledge`.
//!
//! Wraps `ProjectKnowledge` with a vector index for semantic recall.
//! Facts are automatically embedded on `remember` and searched via
//! cosine similarity on `recall`, with hybrid exact + semantic ranking.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::knowledge::{KnowledgeFact, ProjectKnowledge};
use crate::core::embedding_quant::{self, QuantizedVector};
use crate::core::memory_policy::MemoryPolicy;

#[cfg(feature = "dense")]
use super::embeddings::EmbeddingEngine;

const ALPHA_SEMANTIC: f32 = 0.6;
const BETA_CONFIDENCE: f32 = 0.25;
const GAMMA_RECENCY: f32 = 0.15;
const MAX_RECENCY_DAYS: f32 = 90.0;

/// Cosine threshold above which a freshly-remembered fact is treated as a
/// semantic near-duplicate of an existing one. Deliberately conservative — only
/// genuine paraphrases ("DB is Postgres" / "we persist to PostgreSQL") clear it,
/// so the advisory stays signal, not noise. Non-destructive: it nudges the agent
/// to resolve the pair, never auto-merges (distinct facts can be near in embedding
/// space, e.g. "Postgres 14" vs "Postgres 15").
pub const SEMANTIC_DUP_THRESHOLD: f32 = 0.86;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactEmbedding {
    pub category: String,
    pub key: String,
    /// int8-quantized vector (turbovec-derived) — 4× smaller on disk than f32.
    pub quant: QuantizedVector,
}

impl FactEmbedding {
    /// Vector width, which identifies the model that produced it.
    fn width(&self) -> usize {
        self.quant.code.len()
    }

    /// Similarity against a full-precision (L2-normalized) query, scored
    /// directly against the int8 codes.
    fn similarity(&self, query: &[f32]) -> f32 {
        embedding_quant::dot_quant(query, &self.quant)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEmbeddingIndex {
    pub project_hash: String,
    pub entries: Vec<FactEmbedding>,
}

impl KnowledgeEmbeddingIndex {
    pub fn new(project_hash: &str) -> Self {
        Self {
            project_hash: project_hash.to_string(),
            entries: Vec::new(),
        }
    }

    pub fn upsert(&mut self, category: &str, key: &str, embedding: &[f32]) {
        let quant = embedding_quant::quantize(embedding);
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.category == category && e.key == key)
        {
            existing.quant = quant;
        } else {
            self.entries.push(FactEmbedding {
                category: category.to_string(),
                key: key.to_string(),
                quant,
            });
        }
    }

    pub fn remove(&mut self, category: &str, key: &str) {
        self.entries
            .retain(|e| !(e.category == category && e.key == key));
    }

    #[cfg(feature = "dense")]
    pub fn semantic_search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Vec<(&FactEmbedding, f32)> {
        let mut scored: Vec<(&FactEmbedding, f32)> = self
            .entries
            .iter()
            // Entries from a previous model have another width: never compare.
            .filter(|e| e.width() == query_embedding.len())
            .map(|e| {
                let sim = e.similarity(query_embedding);
                (e, sim)
            })
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.category.cmp(&b.0.category))
                .then_with(|| a.0.key.cmp(&b.0.key))
        });
        scored.truncate(top_k);
        scored
    }

    fn index_path(project_hash: &str) -> Option<PathBuf> {
        let dir = crate::core::data_dir::sobre_data_dir()
            .ok()?
            .join("knowledge")
            .join(project_hash);
        Some(dir.join("embeddings.json"))
    }

    pub fn load(project_hash: &str) -> Option<Self> {
        let path = Self::index_path(project_hash)?;
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::index_path(&self.project_hash)
            .ok_or_else(|| "Cannot determine data directory".to_string())?;
        let json = serde_json::to_string(self).map_err(|e| format!("{e}"))?;
        // Atomic write (temp + rename) so a concurrent, lock-free reader in
        // `recall` (which loads the index without taking the per-project lock)
        // never observes a half-written file — it sees either the old or the new
        // complete index, never trailing garbage (issue #412).
        crate::config_io::write_atomic(&path, &json)
    }
}

pub fn reset(project_hash: &str) -> Result<(), String> {
    let path = KnowledgeEmbeddingIndex::index_path(project_hash)
        .ok_or_else(|| "Cannot determine data directory".to_string())?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct ScoredFact<'a> {
    pub fact: &'a KnowledgeFact,
    pub score: f32,
    pub semantic_score: f32,
    pub confidence_score: f32,
    pub recency_score: f32,
}

#[cfg(feature = "dense")]
pub fn semantic_recall<'a>(
    knowledge: &'a ProjectKnowledge,
    index: &KnowledgeEmbeddingIndex,
    engine: &EmbeddingEngine,
    query: &str,
    top_k: usize,
) -> Vec<ScoredFact<'a>> {
    let Ok(query_embedding) = engine.embed_query(query) else {
        return lexical_fallback(knowledge, query, top_k);
    };

    let semantic_hits = index.semantic_search(&query_embedding, top_k * 2);

    let mut results: Vec<ScoredFact<'a>> = Vec::new();

    for (entry, sim) in &semantic_hits {
        if let Some(fact) = knowledge
            .facts
            .iter()
            .find(|f| f.category == entry.category && f.key == entry.key && f.is_current())
        {
            let confidence_score = fact.quality_score();
            let recency_score = recency_decay(fact);
            let score = ALPHA_SEMANTIC * sim
                + BETA_CONFIDENCE * confidence_score
                + GAMMA_RECENCY * recency_score;

            results.push(ScoredFact {
                fact,
                score,
                semantic_score: *sim,
                confidence_score,
                recency_score,
            });
        }
    }

    let exact_matches = knowledge.recall(query);
    for fact in exact_matches {
        let already_included = results
            .iter()
            .any(|r| r.fact.category == fact.category && r.fact.key == fact.key);
        if !already_included {
            results.push(ScoredFact {
                fact,
                score: 1.0,
                semantic_score: 1.0,
                confidence_score: fact.quality_score(),
                recency_score: recency_decay(fact),
            });
        }
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.confidence_score
                    .partial_cmp(&a.confidence_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                b.recency_score
                    .partial_cmp(&a.recency_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.fact.category.cmp(&b.fact.category))
            .then_with(|| a.fact.key.cmp(&b.fact.key))
            .then_with(|| a.fact.value.cmp(&b.fact.value))
    });
    results.truncate(top_k);
    results
}

#[cfg(feature = "dense")]
pub fn semantic_recall_semantic_only<'a>(
    knowledge: &'a ProjectKnowledge,
    index: &KnowledgeEmbeddingIndex,
    engine: &EmbeddingEngine,
    query: &str,
    top_k: usize,
) -> Vec<ScoredFact<'a>> {
    let Ok(query_embedding) = engine.embed_query(query) else {
        return Vec::new();
    };

    let semantic_hits = index.semantic_search(&query_embedding, top_k * 2);
    let mut results: Vec<ScoredFact<'a>> = Vec::new();

    for (entry, sim) in &semantic_hits {
        if let Some(fact) = knowledge
            .facts
            .iter()
            .find(|f| f.category == entry.category && f.key == entry.key && f.is_current())
        {
            let confidence_score = fact.quality_score();
            let recency_score = recency_decay(fact);
            let score = ALPHA_SEMANTIC * sim
                + BETA_CONFIDENCE * confidence_score
                + GAMMA_RECENCY * recency_score;

            results.push(ScoredFact {
                fact,
                score,
                semantic_score: *sim,
                confidence_score,
                recency_score,
            });
        }
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.confidence_score
                    .partial_cmp(&a.confidence_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                b.recency_score
                    .partial_cmp(&a.recency_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.fact.category.cmp(&b.fact.category))
            .then_with(|| a.fact.key.cmp(&b.fact.key))
            .then_with(|| a.fact.value.cmp(&b.fact.value))
    });
    results.truncate(top_k);
    results
}

pub fn compact_against_knowledge(
    index: &mut KnowledgeEmbeddingIndex,
    knowledge: &ProjectKnowledge,
    policy: &MemoryPolicy,
) {
    use std::collections::HashMap;

    let mut current: HashMap<(&str, &str), &KnowledgeFact> = HashMap::new();
    for f in &knowledge.facts {
        if f.is_current() {
            current.insert((f.category.as_str(), f.key.as_str()), f);
        }
    }

    let mut kept: Vec<(FactEmbedding, &KnowledgeFact)> = index
        .entries
        .iter()
        .filter_map(|e| {
            current
                .get(&(e.category.as_str(), e.key.as_str()))
                .map(|f| (e.clone(), *f))
        })
        .collect();

    kept.sort_by(|(ea, fa), (eb, fb)| {
        fb.confidence
            .partial_cmp(&fa.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| fb.last_confirmed.cmp(&fa.last_confirmed))
            .then_with(|| fb.retrieval_count.cmp(&fa.retrieval_count))
            .then_with(|| ea.category.cmp(&eb.category))
            .then_with(|| ea.key.cmp(&eb.key))
    });

    let max = policy.embeddings.max_facts;
    if kept.len() > max {
        kept.truncate(max);
    }

    index.entries = kept.into_iter().map(|(e, _)| e).collect();
}

fn lexical_fallback<'a>(
    knowledge: &'a ProjectKnowledge,
    query: &str,
    top_k: usize,
) -> Vec<ScoredFact<'a>> {
    knowledge
        .recall(query)
        .into_iter()
        .take(top_k)
        .map(|fact| ScoredFact {
            fact,
            score: fact.confidence,
            semantic_score: 0.0,
            confidence_score: fact.confidence,
            recency_score: recency_decay(fact),
        })
        .collect()
}

fn recency_decay(fact: &KnowledgeFact) -> f32 {
    let days_old = chrono::Utc::now()
        .signed_duration_since(fact.last_confirmed)
        .num_days() as f32;
    (1.0 - days_old / MAX_RECENCY_DAYS).max(0.0)
}

#[cfg(feature = "dense")]
pub fn embed_and_store(
    index: &mut KnowledgeEmbeddingIndex,
    engine: &EmbeddingEngine,
    category: &str,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let text = format!("{category} {key}: {value}");
    let embedding = engine.embed(&text).map_err(|e| format!("{e}"))?;
    index.upsert(category, key, &embedding);
    Ok(())
}

/// Per-`remember` cap for [`backfill_missing`]. One MiniLM mini-batch
/// (`embed_batch` chunks at 64) — bounded latency on the write path while an
/// actively-used project converges to full vector coverage within a few calls.
pub const BACKFILL_PER_REMEMBER: usize = 32;

/// Current facts that have no vector in the side-car index, most valuable
/// first (same ordering as `embeddings_reindex`: confidence, then recency).
/// Engine-free so the selection is unit-testable with raw indices.
pub fn missing_current_facts<'a>(
    index: &KnowledgeEmbeddingIndex,
    knowledge: &'a ProjectKnowledge,
    cap: usize,
) -> Vec<&'a KnowledgeFact> {
    use std::collections::HashSet;

    let have: HashSet<(&str, &str)> = index
        .entries
        .iter()
        .map(|e| (e.category.as_str(), e.key.as_str()))
        .collect();

    let mut missing: Vec<&KnowledgeFact> = knowledge
        .facts
        .iter()
        .filter(|f| f.is_current() && !have.contains(&(f.category.as_str(), f.key.as_str())))
        .collect();
    missing.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.last_confirmed.cmp(&a.last_confirmed))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.key.cmp(&b.key))
    });
    missing.truncate(cap);
    missing
}

/// Lazy vector backfill: embeds up to `cap` current facts that are missing
/// from the side-car index. Facts land without vectors on two paths — the
/// consolidation/ETL writers never embed at all, and a non-blocking `remember`
/// skips its side-car while the engine is still warming up. Without a healer
/// those facts stay invisible to `mode=semantic` recall until a *manual*
/// `embeddings_reindex`. Called from `remember` under the per-project lock
/// once the engine is warm, so active projects self-heal incrementally.
/// Returns the number of facts embedded.
#[cfg(feature = "dense")]
pub fn backfill_missing(
    index: &mut KnowledgeEmbeddingIndex,
    engine: &EmbeddingEngine,
    knowledge: &ProjectKnowledge,
    cap: usize,
) -> usize {
    // After a model change, re-embed what the previous model wrote.
    let width = engine.dimensions();
    index.entries.retain(|e| e.width() == width);
    let missing = missing_current_facts(index, knowledge, cap);
    if missing.is_empty() {
        return 0;
    }

    let texts: Vec<String> = missing
        .iter()
        .map(|f| format!("{} {}: {}", f.category, f.key, f.value))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let Ok(vectors) = engine.embed_batch(&refs) else {
        return 0;
    };

    let mut stored = 0usize;
    for (fact, vector) in missing.iter().zip(vectors) {
        index.upsert(&fact.category, &fact.key, &vector);
        stored += 1;
    }
    stored
}

/// Embedding-based near-duplicate detection for `remember`. Mirrors the lexical
/// [`find_cross_key_similar`] but scores cosine similarity, so paraphrases that
/// share few tokens are still caught. Read-only against the *pre-upsert* index,
/// so the incoming fact never matches itself. Returns advisory hits for the
/// agent to resolve — it never mutates or merges facts.
///
/// [`find_cross_key_similar`]: crate::core::knowledge::find_cross_key_similar
#[cfg(feature = "dense")]
pub fn find_semantic_duplicates(
    index: &KnowledgeEmbeddingIndex,
    engine: &EmbeddingEngine,
    knowledge: &ProjectKnowledge,
    new_category: &str,
    new_key: &str,
    new_value: &str,
    threshold: f32,
    limit: usize,
) -> Vec<crate::core::knowledge::SimilarFact> {
    // Cheap static-model embed (microseconds); kept separate from the storage
    // embed in `embed_and_store` so its well-tested side-car path is untouched.
    let text = format!("{new_category} {new_key}: {new_value}");
    let Ok(query) = engine.embed(&text) else {
        return Vec::new();
    };
    semantic_duplicates_from_query(
        index,
        knowledge,
        new_category,
        new_key,
        &query,
        threshold,
        limit,
    )
}

/// Engine-free core of [`find_semantic_duplicates`]: scans an in-memory index
/// for entries whose cosine similarity to a precomputed query embedding clears
/// `threshold`, maps them back to current facts, and excludes the incoming
/// fact's own key and non-current facts. Pure
/// and deterministic so the dedup logic is unit-testable with raw vectors.
fn semantic_duplicates_from_query(
    index: &KnowledgeEmbeddingIndex,
    knowledge: &ProjectKnowledge,
    new_category: &str,
    new_key: &str,
    query: &[f32],
    threshold: f32,
    limit: usize,
) -> Vec<crate::core::knowledge::SimilarFact> {
    use crate::core::knowledge::SimilarFact;

    let mut scored: Vec<(&FactEmbedding, f32)> = index
        .entries
        .iter()
        .filter(|e| !(e.category == new_category && e.key == new_key))
        .filter(|e| e.width() == query.len())
        .map(|e| (e, e.similarity(query)))
        .filter(|(_, sim)| *sim >= threshold)
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.category.cmp(&b.0.category))
            .then_with(|| a.0.key.cmp(&b.0.key))
    });

    let mut out: Vec<SimilarFact> = Vec::new();
    for (entry, sim) in scored {
        let Some(fact) = knowledge
            .facts
            .iter()
            .find(|f| f.category == entry.category && f.key == entry.key && f.is_current())
        else {
            continue;
        };
        let preview = if fact.value.len() > 60 {
            format!("{}...", &fact.value[..fact.value.floor_char_boundary(57)])
        } else {
            fact.value.clone()
        };
        out.push(SimilarFact {
            category: fact.category.clone(),
            key: fact.key.clone(),
            value_preview: preview,
            similarity: sim,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::knowledge::KnowledgeArchetype;

    fn fact_with(category: &str, key: &str, source: &str) -> KnowledgeFact {
        let now = chrono::Utc::now();
        KnowledgeFact {
            category: category.to_string(),
            key: key.to_string(),
            value: "v".to_string(),
            source_session: source.to_string(),
            confidence: 0.8,
            created_at: now,
            last_confirmed: now,
            retrieval_count: 0,
            last_retrieved: None,
            valid_from: None,
            valid_until: None,
            supersedes: None,
            confirmation_count: 0,
            feedback_up: 0,
            feedback_down: 0,
            last_feedback: None,
            sensitivity: crate::core::sensitivity::SensitivityLevel::default(),
            imported_from: None,
            archetype: KnowledgeArchetype::infer_from_category(category),
            revision_count: 0,
        }
    }

    #[test]
    fn reset_removes_index_file() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        crate::test_env::set_var("SOBRE_DATA_DIR", tmp.path().to_string_lossy().to_string());

        let idx = KnowledgeEmbeddingIndex {
            project_hash: "projhash".to_string(),
            entries: vec![FactEmbedding {
                category: "arch".to_string(),
                key: "db".to_string(),
                quant: embedding_quant::quantize(&[1.0, 0.0, 0.0]),
            }],
        };
        idx.save().expect("save");
        assert!(KnowledgeEmbeddingIndex::load("projhash").is_some());

        reset("projhash").expect("reset");
        assert!(KnowledgeEmbeddingIndex::load("projhash").is_none());

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn concurrent_remember_keeps_all_embeddings() {
        // #412: the embedding-index read-modify-write must be serialized under
        // the per-project lock and compacted against fresh on-disk knowledge.
        // The old lock-free + stale-snapshot path let parallel writers clobber
        // each other's vectors and prune just-stored ones. This mirrors
        // `handle_remember`'s locked path (raw vectors, so no embedding engine
        // is needed) and asserts every concurrently-stored embedding survives.
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        crate::test_env::set_var("SOBRE_DATA_DIR", tmp.path().to_string_lossy().to_string());

        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).expect("mkdir");
        let project_root = project.to_string_lossy().to_string();

        const N: usize = 16;
        let mut handles = Vec::with_capacity(N);
        for i in 0..N {
            let root = project_root.clone();
            handles.push(std::thread::spawn(move || {
                let policy = MemoryPolicy::default();
                let cat = "arch";
                let key = format!("k{i}");
                // 1) Commit the fact under the lock (as handle_remember does).
                let (knowledge, ()) = ProjectKnowledge::mutate_locked(&root, |kn| {
                    kn.remember(cat, &key, "v", "s", 0.9, &policy);
                })
                .expect("commit fact");
                // 2) Embedding side-car under the SAME lock + fresh-knowledge
                //    compaction — exactly the fixed handle_remember path.
                ProjectKnowledge::with_project_lock(&root, || {
                    let mut idx = KnowledgeEmbeddingIndex::load(&knowledge.project_hash)
                        .unwrap_or_else(|| KnowledgeEmbeddingIndex::new(&knowledge.project_hash));
                    idx.upsert(cat, &key, &[1.0, 0.0, 0.0]);
                    let fresh = ProjectKnowledge::load(&root);
                    let kref = fresh.as_ref().unwrap_or(&knowledge);
                    compact_against_knowledge(&mut idx, kref, &policy);
                    idx.save().expect("save index");
                });
            }));
        }
        for h in handles {
            h.join().expect("thread join");
        }

        let knowledge = ProjectKnowledge::load(&project_root).expect("knowledge persisted");
        let current = knowledge.facts.iter().filter(|f| f.is_current()).count();
        assert_eq!(current, N, "all {N} facts must be committed");

        let idx = KnowledgeEmbeddingIndex::load(&knowledge.project_hash).expect("index persisted");
        assert_eq!(
            idx.entries.len(),
            N,
            "every concurrently-stored embedding must survive (got {})",
            idx.entries.len()
        );

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn compact_drops_missing_or_archived_facts() {
        let mut knowledge = ProjectKnowledge::new("/tmp/project");
        let now = chrono::Utc::now();
        knowledge.facts.push(KnowledgeFact {
            category: "arch".to_string(),
            key: "db".to_string(),
            value: "Postgres".to_string(),
            source_session: "s".to_string(),
            confidence: 0.9,
            created_at: now,
            last_confirmed: now,
            retrieval_count: 5,
            last_retrieved: None,
            valid_from: None,
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
        });
        knowledge.facts.push(KnowledgeFact {
            category: "arch".to_string(),
            key: "old".to_string(),
            value: "Old".to_string(),
            source_session: "s".to_string(),
            confidence: 0.9,
            created_at: now,
            last_confirmed: now,
            retrieval_count: 0,
            last_retrieved: None,
            valid_from: None,
            valid_until: Some(now),
            supersedes: None,
            confirmation_count: 1,
            feedback_up: 0,
            feedback_down: 0,
            last_feedback: None,
            sensitivity: crate::core::sensitivity::SensitivityLevel::default(),
            imported_from: None,
            archetype: KnowledgeArchetype::default(),
            revision_count: 0,
        });

        let mut idx = KnowledgeEmbeddingIndex::new(&knowledge.project_hash);
        idx.upsert("arch", "db", &[1.0, 0.0, 0.0]);
        idx.upsert("arch", "old", &[0.0, 1.0, 0.0]);
        idx.upsert("ops", "deploy", &[0.0, 0.0, 1.0]);

        compact_against_knowledge(&mut idx, &knowledge, &MemoryPolicy::default());
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].category, "arch");
        assert_eq!(idx.entries[0].key, "db");
    }

    #[test]
    fn missing_current_facts_selects_unindexed_by_confidence_capped() {
        let mut knowledge = ProjectKnowledge::new("/tmp/project");

        let indexed = fact_with("arch", "indexed", "s");
        let mut high = fact_with("arch", "high", "s");
        high.confidence = 0.95;
        let mut low = fact_with("arch", "low", "s");
        low.confidence = 0.5;
        let mut archived = fact_with("arch", "archived", "s");
        archived.valid_until = Some(chrono::Utc::now());
        knowledge.facts.extend([indexed, high, low, archived]);

        let mut idx = KnowledgeEmbeddingIndex::new("test");
        idx.upsert("arch", "indexed", &[1.0, 0.0, 0.0]);

        // Already-indexed and archived facts are excluded; rest ordered by
        // confidence (the reindex ordering).
        let missing = missing_current_facts(&idx, &knowledge, 10);
        let keys: Vec<&str> = missing.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, vec!["high", "low"]);

        // Cap bounds the batch (most valuable first).
        let capped = missing_current_facts(&idx, &knowledge, 1);
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].key, "high");

        // Full coverage → nothing to backfill.
        idx.upsert("arch", "high", &[0.0, 1.0, 0.0]);
        idx.upsert("arch", "low", &[0.0, 0.0, 1.0]);
        assert!(missing_current_facts(&idx, &knowledge, 10).is_empty());
    }

    #[test]
    fn index_upsert_and_remove() {
        let mut idx = KnowledgeEmbeddingIndex::new("test");
        idx.upsert("arch", "db", &[1.0, 0.0, 0.0]);
        assert_eq!(idx.entries.len(), 1);

        idx.upsert("arch", "db", &[0.0, 1.0, 0.0]);
        assert_eq!(idx.entries.len(), 1);
        // Stored quantized now: the dominant axis reconstructs to ~1.0.
        let recon = idx.entries[0].quant.dequantize();
        assert!((recon[1] - 1.0).abs() < 1e-6);

        idx.upsert("arch", "cache", &[0.0, 0.0, 1.0]);
        assert_eq!(idx.entries.len(), 2);

        idx.remove("arch", "db");
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].key, "cache");
    }

    #[test]
    fn recency_decay_recent() {
        let fact = KnowledgeFact {
            category: "test".to_string(),
            key: "k".to_string(),
            value: "v".to_string(),
            source_session: "s".to_string(),
            confidence: 0.9,
            created_at: chrono::Utc::now(),
            last_confirmed: chrono::Utc::now(),
            retrieval_count: 0,
            last_retrieved: None,
            valid_from: None,
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
        };
        let decay = recency_decay(&fact);
        assert!(
            decay > 0.95,
            "Recent fact should have high recency: {decay}"
        );
    }

    #[test]
    fn recency_decay_old() {
        let old_date = chrono::Utc::now() - chrono::Duration::days(100);
        let fact = KnowledgeFact {
            category: "test".to_string(),
            key: "k".to_string(),
            value: "v".to_string(),
            source_session: "s".to_string(),
            confidence: 0.5,
            created_at: old_date,
            last_confirmed: old_date,
            retrieval_count: 0,
            last_retrieved: None,
            valid_from: None,
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
        };
        let decay = recency_decay(&fact);
        assert_eq!(decay, 0.0, "100-day-old fact should have 0 recency");
    }

    #[cfg(feature = "dense")]
    #[test]
    fn semantic_search_skips_other_widths() {
        let mut idx = KnowledgeEmbeddingIndex::new("test");
        idx.upsert("arch", "db", &[1.0, 0.0, 0.0]);
        idx.upsert("arch", "old", &[1.0, 0.0]);
        let results = idx.semantic_search(&[1.0, 0.0, 0.0], 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.key, "db");
    }

    #[cfg(feature = "dense")]
    #[test]
    fn semantic_search_ranking() {
        let mut idx = KnowledgeEmbeddingIndex::new("test");
        idx.upsert("arch", "db", &[1.0, 0.0, 0.0]);
        idx.upsert("arch", "cache", &[0.0, 1.0, 0.0]);
        idx.upsert("ops", "deploy", &[0.5, 0.5, 0.0]);

        let query = vec![1.0, 0.0, 0.0];
        let results = idx.semantic_search(&query, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.key, "db");
    }

    #[test]
    fn semantic_dup_flags_high_cosine_other_key() {
        let policy = MemoryPolicy::default();
        let mut kn = ProjectKnowledge::new("/tmp/semdup-1");
        kn.remember(
            "arch",
            "db",
            "PostgreSQL is the primary database",
            "s",
            0.9,
            &policy,
        );
        kn.remember(
            "arch",
            "cache",
            "Redis is the cache layer",
            "s",
            0.9,
            &policy,
        );

        let mut idx = KnowledgeEmbeddingIndex::new(&kn.project_hash);
        idx.upsert("arch", "db", &[1.0, 0.0, 0.0]);
        idx.upsert("arch", "cache", &[0.0, 1.0, 0.0]);

        // Query near-identical to the "db" entry, remembering a *different* key.
        let query = [1.0, 0.0, 0.0];
        let dups = semantic_duplicates_from_query(&idx, &kn, "arch", "database", &query, 0.86, 3);
        assert_eq!(
            dups.len(),
            1,
            "only the near-identical entry clears threshold"
        );
        assert_eq!(dups[0].key, "db");
        assert!(dups[0].similarity >= 0.86);
    }

    #[test]
    fn semantic_dup_excludes_self_and_judged() {
        let policy = MemoryPolicy::default();
        let mut kn = ProjectKnowledge::new("/tmp/semdup-2");
        kn.remember(
            "arch",
            "db",
            "PostgreSQL primary database",
            "s",
            0.9,
            &policy,
        );

        let mut idx = KnowledgeEmbeddingIndex::new(&kn.project_hash);
        idx.upsert("arch", "db", &[1.0, 0.0, 0.0]);
        let query = [1.0, 0.0, 0.0];

        // Self: remembering the same key must never flag itself.
        let self_hits = semantic_duplicates_from_query(&idx, &kn, "arch", "db", &query, 0.86, 3);
        assert!(self_hits.is_empty(), "a fact is never its own duplicate");

        // A different key matches.
        let other = semantic_duplicates_from_query(&idx, &kn, "arch", "database", &query, 0.86, 3);
        assert_eq!(other.len(), 1);
    }
}
