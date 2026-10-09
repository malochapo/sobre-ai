//! Dense & hybrid ranking (one pipeline for every caller), inline-embed
//! budget, engine + embedding index loading, embedding alignment.

use std::path::Path;

use crate::core::bm25_index::BM25Index;
use crate::core::embeddings::EmbeddingEngine;
use crate::core::hnsw::FlatEmbeddings;
use crate::core::hybrid_search::{HybridConfig, HybridResult, format_hybrid_results};

#[allow(clippy::wildcard_imports)]
use super::*;

/// Max chunks the hybrid/dense path will embed *inline* (under the
/// per-request watchdog) before degrading instead of embedding. A server that
/// started before the on-disk dense index existed would otherwise embed the
/// whole corpus on the first query — observed as a runaway 500%+ CPU child the
/// 120s watchdog abandons but cannot cancel.
pub(crate) fn inline_embed_max_chunks() -> usize {
    2000
}

/// Pure budget check for the cold-start guard: `max == 0` disables it,
/// and the budget is inclusive (`pending == max` still embeds inline).
pub(crate) fn exceeds_inline_embed_budget(pending: usize, max: usize) -> bool {
    max > 0 && pending > max
}

/// Decide whether this call would trigger a large inline embed the watchdog
/// cannot safely bound. Returns the pending-chunk count when the call
/// should degrade instead of embedding inline; `None` keeps the normal path
/// (warm index, or an incremental embed of only a few changed chunks).
pub(crate) fn cold_start_embed_guard(
    embed_idx: &EmbeddingIndex,
    index: &BM25Index,
) -> Option<usize> {
    let pending = embed_idx.pending_chunk_count(&index.chunks);
    exceeds_inline_embed_budget(pending, inline_embed_max_chunks()).then_some(pending)
}

/// One-line, deterministic hint pointing at the out-of-band dense build. Shared
/// by the hybrid fallback and the dense fail-fast so the guidance never drifts.
pub(crate) fn dense_build_hint(pending: usize, compact: bool) -> String {
    if compact {
        format!("[dense not built: {pending} chunks pending — run: sobre index build]")
    } else {
        format!(
            "[sobre: dense index not built ({pending} chunks would embed inline). \
             Build it once — no per-query embed, no cold-start hang: \
             sobre index build]"
        )
    }
}

/// One ranked answer from the dense pipeline.
pub(crate) struct Ranked {
    pub results: Vec<HybridResult>,
    /// Share of chunks with an embedding, 0.0 when the dense signal is absent.
    pub coverage: f64,
    /// Set when hybrid ran without its dense signal (the dense index is not
    /// built yet): what happened and how to fix it, for the caller to show.
    pub degraded: Option<String>,
}

fn filter_pred(filter: &SearchFilter) -> impl Fn(&str) -> bool + '_ {
    |p: &str| filter.matches(p)
}

/// Hybrid ranking: BM25 + dense embeddings + recently-touched-file graph
/// neighbors, fused by RRF, then SPLADE and complexity boosts. Every hybrid
/// search (one root, workspace, `find_related`, the eval harness) goes through
/// here.
///
/// On a cold dense index it never embeds the whole corpus inline: it
/// ranks without the dense signal and says so in [`Ranked::degraded`].
pub(crate) fn hybrid_ranked(
    query: &str,
    root: &Path,
    index: &BM25Index,
    top_k: usize,
    filter: &SearchFilter,
    cfg: &HybridConfig,
) -> Result<Ranked, String> {
    let (engine, mut embed_idx) = load_engine_and_index(root)?;
    let candidate_k = filtered_candidate_k(top_k, filter.is_active());
    let graph_ranks = graph_rrf_ranks_for_search_root(root);

    let (mut results, coverage, degraded) =
        if let Some(pending) = cold_start_embed_guard(&embed_idx, index) {
            let mut results = crate::core::hybrid_search::hybrid_search(
                query,
                index,
                None,
                None,
                candidate_k,
                cfg,
                graph_ranks.as_ref(),
            );
            if filter.is_active() {
                results.retain(|r| filter.matches(&r.file_path));
            }
            (results, 0.0, Some(dense_build_hint(pending, true)))
        } else {
            let (aligned, coverage) = ensure_embeddings(root, index, engine, &mut embed_idx)?;
            let pred = filter_pred(filter);
            let results = crate::core::dense_backend::hybrid_results(
                index,
                engine,
                &aligned,
                query,
                candidate_k,
                cfg,
                filter.is_active().then_some(&pred as &dyn Fn(&str) -> bool),
                graph_ranks.as_ref(),
            );
            (results, coverage, None)
        };

    if cfg.splade_weight > 0.0 {
        let splade = crate::core::splade_retrieval::hybrid_retrieve(query, index, candidate_k);
        boost_with_splade(&mut results, &splade, cfg.splade_weight);
    }
    boost_with_complexity(&mut results, root.to_str().unwrap_or(""), 0.3);
    results.truncate(top_k);
    Ok(Ranked {
        results,
        coverage,
        degraded,
    })
}

/// Dense-only ranking. A cold dense index is an error: there is nothing to
/// fall back to that would still be "dense".
pub(crate) fn dense_ranked(
    query: &str,
    root: &Path,
    index: &BM25Index,
    top_k: usize,
    filter: &SearchFilter,
) -> Result<Ranked, String> {
    let (engine, mut embed_idx) = load_engine_and_index(root)?;
    if let Some(pending) = cold_start_embed_guard(&embed_idx, index) {
        return Err(dense_build_hint(pending, true));
    }
    let (aligned, coverage) = ensure_embeddings(root, index, engine, &mut embed_idx)?;
    let pred = filter_pred(filter);
    let mut results = crate::core::dense_backend::dense_results_as_hybrid(
        index,
        engine,
        &aligned,
        query,
        filtered_candidate_k(top_k, filter.is_active()),
        filter.is_active().then_some(&pred as &dyn Fn(&str) -> bool),
    )?;
    results.truncate(top_k);
    Ok(Ranked {
        results,
        coverage,
        degraded: None,
    })
}

/// Render a dense or hybrid answer for one root.
pub(crate) fn render_ranked(
    mode: SearchMode,
    query: &str,
    index: &BM25Index,
    top_k: usize,
    compact: bool,
    ranked: &Ranked,
) -> String {
    let n = ranked.results.len();
    let engine = match (&ranked.degraded, mode) {
        (Some(_), _) => "bm25+graph, dense index not built",
        (None, SearchMode::Dense) => "dense",
        (None, _) => "hybrid",
    };
    let mut out = if compact {
        let cov = if ranked.degraded.is_some() {
            String::new()
        } else {
            format!(", embed_cov={:.0}%", ranked.coverage * 100.0)
        };
        format!(
            "semantic_search({engine},{top_k}) → {n} results, {} chunks{cov}\n",
            index.doc_count
        )
    } else {
        let cov = if ranked.degraded.is_some() {
            String::new()
        } else {
            format!(", embeddings coverage {:.0}%", ranked.coverage * 100.0)
        };
        format!(
            "Semantic search ({engine}): \"{}\" ({n} results from {} indexed chunks{cov})\n",
            truncate_query(query, 60),
            index.doc_count
        )
    };
    out.push_str(&format_hybrid_results(&ranked.results, compact));
    if let Some(note) = &ranked.degraded {
        out.push('\n');
        out.push_str(note);
    }
    out
}

fn reject_under_hard_pressure(operation: &str) -> Result<(), String> {
    if crate::core::memory_guard::abort_requested() {
        Err(format!("{operation} cancelled during hard memory pressure"))
    } else {
        Ok(())
    }
}

pub(crate) fn load_engine_and_index(
    root: &Path,
) -> Result<(&'static EmbeddingEngine, EmbeddingIndex), String> {
    reject_under_hard_pressure("semantic embedding load")?;

    let engine = crate::core::embeddings::shared_engine()
        .ok_or_else(|| "embedding engine load failed".to_string())?;

    let model_name = engine.model_name();
    let mut idx = EmbeddingIndex::load(root)
        .unwrap_or_else(|| EmbeddingIndex::new_with_model(engine.dimensions(), model_name));

    if let Some((stored, current)) = idx.model_mismatch(model_name) {
        tracing::warn!(
            "[embeddings] model changed: {stored} → {current}. Re-indexing all embeddings."
        );
        idx = EmbeddingIndex::new_with_model(engine.dimensions(), model_name);
    } else if idx.dimension_mismatch(engine.dimensions()) {
        tracing::warn!(
            "[embeddings] dimension mismatch: index={}d, engine={}d. Re-indexing.",
            idx.dimensions,
            engine.dimensions()
        );
        idx = EmbeddingIndex::new_with_model(engine.dimensions(), model_name);
    }

    Ok((engine, idx))
}

/// Aligned embedding corpus as a single contiguous [`FlatEmbeddings`] allocation,
/// plus coverage. The flat row-major
/// layout gives sequential memory access during dot-product scoring — one
/// dereference instead of the two-level indirection of `Arc<[Vec<f32>]>`.
pub(crate) type AlignedEmbeddings = (FlatEmbeddings, f64);

pub(crate) fn ensure_embeddings(
    root: &Path,
    index: &BM25Index,
    engine: &EmbeddingEngine,
    embed_idx: &mut EmbeddingIndex,
) -> Result<AlignedEmbeddings, String> {
    // A resident index whose bodies were shrunk to snippets (post-embedding RAM
    // reclaim) must NEVER drive re-embedding: `files_needing_update` hashes
    // `c.content`, so truncated bodies would falsely flag every file as changed
    // and re-embed 5-line snippets over the full-body vectors persisted earlier
    // this session. Embeddings for exactly these chunks were already built and
    // saved before truncation, and alignment is keyed by (path, start, end) —
    // not content — so we just re-align here. If a file genuinely changed, the
    // BM25 cache fingerprint goes stale and a fresh full-content index (reloaded
    // from disk) replaces this one, restoring the normal re-embed path.
    if index.content_truncated {
        let aligned = embed_idx.get_aligned_flat(&index.chunks).ok_or_else(|| {
            "embedding alignment failed on truncated resident index; \
                 refusing to re-embed snippet-only bodies"
                .to_string()
        })?;
        let coverage = embed_idx.coverage(index.chunks.len());
        return Ok((aligned, coverage));
    }

    let mut changed_files = embed_idx.files_needing_update(&index.chunks);
    changed_files.sort();
    changed_files.dedup();

    if !changed_files.is_empty() {
        let changed_set: std::collections::HashSet<&str> = changed_files
            .iter()
            .map(std::string::String::as_str)
            .collect();

        let mut changed_indices: Vec<usize> = Vec::new();
        let mut changed_texts: Vec<&str> = Vec::new();
        for (i, c) in index.chunks.iter().enumerate() {
            if changed_set.contains(c.file_path.as_str()) {
                changed_indices.push(i);
                changed_texts.push(&c.content);
            }
        }

        let batch_embeddings = engine
            .embed_batch(&changed_texts)
            .map_err(|e| format!("batch embed failed: {e}"))?;
        reject_under_hard_pressure("embedding update")?;

        let new_embeddings: Vec<(usize, Vec<f32>)> =
            changed_indices.into_iter().zip(batch_embeddings).collect();

        embed_idx.update(&index.chunks, &new_embeddings, &changed_files, None);
        embed_idx
            .save(root)
            .map_err(|e| format!("save embeddings failed: {e}"))?;
    }

    if let Some(aligned) = embed_idx.get_aligned_flat(&index.chunks) {
        let coverage = embed_idx.coverage(index.chunks.len());
        return Ok((aligned, coverage));
    }

    // Alignment missing: rebuild everything once via batched inference.
    let mut all_files: Vec<String> = index.chunks.iter().map(|c| c.file_path.clone()).collect();
    all_files.sort();
    all_files.dedup();

    let all_texts: Vec<&str> = index.chunks.iter().map(|c| c.content.as_str()).collect();
    let batch_embeddings = engine
        .embed_batch(&all_texts)
        .map_err(|e| format!("batch embed failed: {e}"))?;
    reject_under_hard_pressure("embedding rebuild")?;

    let new_embeddings: Vec<(usize, Vec<f32>)> = batch_embeddings.into_iter().enumerate().collect();

    embed_idx.update(&index.chunks, &new_embeddings, &all_files, None);
    embed_idx
        .save(root)
        .map_err(|e| format!("save embeddings failed: {e}"))?;

    let aligned = embed_idx
        .get_aligned_flat(&index.chunks)
        .ok_or_else(|| "embedding alignment failed after full rebuild".to_string())?;
    let coverage = embed_idx.coverage(index.chunks.len());
    Ok((aligned, coverage))
}
