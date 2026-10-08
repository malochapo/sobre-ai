//! Persists derived artifacts (BM25 chunks, cross-source edges, knowledge
//! facts) into the project stores. The code-health fabric is the producer.

use crate::core::content_chunk::ContentChunk;
use crate::core::graph_index::IndexEdge;

/// A knowledge fact ready for `ProjectKnowledge::remember()`.
#[derive(Debug, Clone)]
pub struct ExtractedFact {
    pub category: String,
    pub key: String,
    pub value: String,
    pub confidence: f32,
}

/// Artifacts to persist — no side effects until [`apply_artifacts_to_stores`].
#[derive(Debug, Clone, Default)]
pub struct ConsolidationArtifacts {
    pub bm25_chunks: Vec<ContentChunk>,
    pub edges: Vec<IndexEdge>,
    pub facts: Vec<ExtractedFact>,
}

fn write_edges_to_property_graph(pg: &crate::core::property_graph::CodeGraph, edges: &[IndexEdge]) {
    // Cross-source edges live in their own table (#682) so external URIs never
    // pollute the File-node catalog and the exact relation kind + weight survive.
    for edge in edges {
        let _ = pg.upsert_cross_source_edge(&edge.from, &edge.to, &edge.kind, edge.weight);
    }
}

/// Names a prior fan-out pass to evict from each store before re-ingesting, so a
/// recomputed source *replaces* rather than *appends to* its previous output.
/// All-`None` (the [`Default`]) is purely additive.
#[derive(Debug, Clone, Default)]
pub struct PrunePrior {
    /// Remove BM25 chunks whose `file_path` starts with this prefix (e.g. `health://`).
    pub bm25_prefix: Option<String>,
    /// Remove cross-source edges of this `kind` (e.g. `health_hotspot`).
    pub edge_kind: Option<String>,
    /// Remove knowledge facts of this `category` (e.g. `code_health`).
    pub fact_category: Option<String>,
}

/// Persist consolidation artifacts into the on-disk stores (BM25 index, property
/// graph, knowledge).
///
/// When `prune` names a prior pass, that pass is evicted from each store *before*
/// the new artifacts are written, so a recomputed source (e.g. the code-health
/// fabric) never leaves resolved signals behind. With the default (all-`None`)
/// prune the behaviour is purely additive.
///
/// Best-effort per store; never panics. Safe to call from a background thread.
pub fn apply_artifacts_to_stores(
    artifacts: &ConsolidationArtifacts,
    project_root: &str,
    prune: &PrunePrior,
) {
    let root_path = std::path::Path::new(project_root);

    // BM25: optionally evict the prior pass, then ingest the current chunks.
    let bm25_prefix = prune.bm25_prefix.as_deref();
    if bm25_prefix.is_some() || !artifacts.bm25_chunks.is_empty() {
        let mut index = crate::core::bm25_index::BM25Index::load_or_build(root_path);
        let removed = bm25_prefix.map_or(0, |p| index.remove_chunks_with_prefix(p));
        let ingested = index.ingest_content_chunks(artifacts.bm25_chunks.clone());
        if (removed > 0 || ingested > 0) && index.save(root_path).is_err() {
            tracing::warn!("[consolidation] BM25 save failed");
        }
    }

    // Cross-source edges → property graph (#682). Evict prior edges of this kind
    // first so a replace-source's resolved links disappear from `sobre_read` hints.
    if prune.edge_kind.is_some() || !artifacts.edges.is_empty() {
        match crate::core::property_graph::CodeGraph::open(project_root) {
            Ok(pg) => {
                if let Some(kind) = &prune.edge_kind {
                    let _ = pg.delete_cross_source_edges_by_kind(kind);
                }
                write_edges_to_property_graph(&pg, &artifacts.edges);
            }
            Err(e) => tracing::warn!("[consolidation] property graph open failed: {e}"),
        }
    }

    // Knowledge: evict the prior category, then remember the current facts.
    // MUST go through `mutate_locked`: this runs on a background thread, and a
    // load → modify → blind save here raced parallel `sobre_knowledge remember`
    // calls — the stale snapshot clobbered facts the agent had just committed
    // (lost update, the exact #326 failure mode).
    if prune.fact_category.is_some() || !artifacts.facts.is_empty() {
        let policy = crate::core::memory_policy::MemoryPolicy::default();

        // Replace-sources (prune) use a stable session id so repeated passes stay
        // byte-identical (#498); additive ingests keep a unique id.
        let session_id = match &prune.fact_category {
            Some(category) => format!("fabric:{category}"),
            None => format!("provider-ingest-{}", chrono::Utc::now().timestamp()),
        };
        let saved =
            crate::core::knowledge::ProjectKnowledge::mutate_locked(project_root, |knowledge| {
                if let Some(category) = &prune.fact_category {
                    knowledge.facts.retain(|f| &f.category != category);
                }
                for fact in &artifacts.facts {
                    knowledge.remember(
                        &fact.category,
                        &fact.key,
                        &fact.value,
                        &session_id,
                        fact.confidence,
                        &policy,
                    );
                }
            });
        if saved.is_err() {
            tracing::warn!("[consolidation] knowledge save failed");
        }
    }
}
