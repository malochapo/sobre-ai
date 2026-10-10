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
    // Cross-source edges live in their own table so external URIs never
    // pollute the File-node catalog and the exact relation kind + weight survive.
    for edge in edges {
        let _ = pg.upsert_cross_source_edge(&edge.from, &edge.to, &edge.kind, edge.weight);
    }
}

/// Names the prior pass to evict from each store before re-ingesting, so a
/// recomputed source *replaces* its previous output.
#[derive(Debug, Clone)]
pub struct PrunePrior {
    /// Remove BM25 chunks whose `file_path` starts with this prefix (e.g. `health://`).
    pub bm25_prefix: String,
    /// Remove cross-source edges of this `kind` (e.g. `health_hotspot`).
    pub edge_kind: String,
    /// Remove knowledge facts of this `category` (e.g. `code_health`).
    pub fact_category: String,
}

/// Persist consolidation artifacts into the on-disk stores (BM25 index, property
/// graph, knowledge).
///
/// The prior pass named by `prune` is evicted from each store *before* the new
/// artifacts are written, so a recomputed source (the code-health fabric) never
/// leaves resolved signals behind.
///
/// Best-effort per store; never panics. Safe to call from a background thread.
pub fn apply_artifacts_to_stores(
    artifacts: &ConsolidationArtifacts,
    project_root: &str,
    prune: &PrunePrior,
) {
    let root_path = std::path::Path::new(project_root);

    // BM25: evict the prior pass, then ingest the current chunks.
    let mut index = crate::core::bm25_index::BM25Index::load_or_build(root_path);
    let removed = index.remove_chunks_with_prefix(&prune.bm25_prefix);
    let ingested = index.ingest_content_chunks(artifacts.bm25_chunks.clone());
    if (removed > 0 || ingested > 0) && index.save(root_path).is_err() {
        tracing::warn!("[consolidation] BM25 save failed");
    }

    // Cross-source edges → property graph. Evict prior edges of this kind
    // first so resolved links disappear from `sobre_read` hints.
    match crate::core::property_graph::CodeGraph::open(project_root) {
        Ok(pg) => {
            let _ = pg.delete_cross_source_edges_by_kind(&prune.edge_kind);
            write_edges_to_property_graph(&pg, &artifacts.edges);
        }
        Err(e) => tracing::warn!("[consolidation] property graph open failed: {e}"),
    }

    // Knowledge: evict the prior category, then remember the current facts.
    // MUST go through `mutate_locked`: this runs on a background thread, and a
    // load → modify → save outside the lock would race parallel
    // `sobre_knowledge remember` calls and clobber facts the agent just
    // committed. A stable session id keeps repeated passes byte-identical.
    let policy = crate::core::memory_policy::MemoryPolicy::default();
    let category = &prune.fact_category;
    let session_id = format!("fabric:{category}");
    let saved = crate::core::knowledge::ProjectKnowledge::mutate_locked(project_root, |knowledge| {
        knowledge.facts.retain(|f| &f.category != category);
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
    if let Err(e) = saved {
        tracing::warn!("[consolidation] knowledge not saved: {e}");
    }
}
