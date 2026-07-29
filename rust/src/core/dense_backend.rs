

/// Which dense backend served a query.
///
/// sobre has exactly one: an in-process HNSW index over locally computed
/// embeddings. The Qdrant and pgvector variants went with invariant 4 — a
/// remote vector database is a service someone has to run. The enum survives so
/// the search path can still report which backend answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DenseBackendKind {
    #[default]
    Local,
}

impl DenseBackendKind {
    /// Honours `LEANCTX_DENSE_BACKEND` so an explicit `local` keeps working and
    /// anything else fails loudly instead of silently falling back.
    ///
    /// # Errors
    /// When the variable names a backend this build does not have.
    pub fn try_from_env() -> Result<Self, String> {
        match std::env::var("LEANCTX_DENSE_BACKEND")
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
            .as_deref()
        {
            None | Some("local") => Ok(Self::Local),
            Some(other) => Err(format!(
                "Unknown LEANCTX_DENSE_BACKEND={other:?} — sobre only has the local, in-process backend"
            )),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Local => "local",
        }
    }
}

#[cfg(feature = "embeddings")]
#[allow(clippy::too_many_arguments)]
pub fn dense_results_as_hybrid(
    backend: DenseBackendKind,
    root: &Path,
    index: &BM25Index,
    engine: &crate::core::embeddings::EmbeddingEngine,
    aligned_embeddings: &FlatEmbeddings,
    changed_files: &[String],
    query: &str,
    top_k: usize,
    filter: Option<&dyn Fn(&str) -> bool>,
) -> Result<Vec<HybridResult>, String> {
    let dense = dense_results(
        backend,
        root,
        index,
        engine,
        aligned_embeddings,
        changed_files,
        query,
        top_k,
        filter,
    )?;

    Ok(dense
        .into_iter()
        .map(|d| HybridResult {
            file_path: d.file_path,
            symbol_name: d.symbol_name,
            kind: d.kind,
            start_line: d.start_line,
            end_line: d.end_line,
            snippet: d.snippet,
            rrf_score: d.similarity as f64,
            bm25_score: None,
            dense_score: Some(d.similarity),
            bm25_rank: None,
            dense_rank: None,
        })
        .collect())
}

#[cfg(feature = "embeddings")]
#[allow(clippy::too_many_arguments)]
pub fn hybrid_results(
    backend: DenseBackendKind,
    root: &Path,
    index: &BM25Index,
    engine: &crate::core::embeddings::EmbeddingEngine,
    aligned_embeddings: &FlatEmbeddings,
    changed_files: &[String],
    query: &str,
    top_k: usize,
    config: &HybridConfig,
    filter: Option<&dyn Fn(&str) -> bool>,
    graph_file_ranks: Option<&std::collections::HashMap<String, usize>>,
) -> Result<Vec<HybridResult>, String> {
    match backend {
        DenseBackendKind::Local => {
            let _ = (root, changed_files);
            let mut results = crate::core::hybrid_search::hybrid_search(
                query,
                index,
                Some(engine),
                Some(aligned_embeddings),
                top_k,
                config,
                graph_file_ranks,
            );
            if let Some(pred) = filter {
                results.retain(|r| pred(&r.file_path));
            }
            results.truncate(top_k);
            Ok(results)
        }
    }
}

#[cfg(feature = "embeddings")]
#[allow(clippy::too_many_arguments)]
fn dense_results(
    backend: DenseBackendKind,
    root: &Path,
    index: &BM25Index,
    engine: &crate::core::embeddings::EmbeddingEngine,
    aligned_embeddings: &FlatEmbeddings,
    changed_files: &[String],
    query: &str,
    top_k: usize,
    filter: Option<&dyn Fn(&str) -> bool>,
) -> Result<Vec<DenseSearchResult>, String> {
    match backend {
        DenseBackendKind::Local => {
            let _ = (root, changed_files);
            dense_results_local(index, engine, aligned_embeddings, query, top_k, filter)
        }
    }
}

#[cfg(feature = "embeddings")]
fn dense_results_local(
    index: &BM25Index,
    engine: &crate::core::embeddings::EmbeddingEngine,
    aligned_embeddings: &FlatEmbeddings,
    query: &str,
    top_k: usize,
    filter: Option<&dyn Fn(&str) -> bool>,
) -> Result<Vec<DenseSearchResult>, String> {
    let query_embedding = engine
        .embed_query(query)
        .map_err(|e| format!("embedding failed: {e}"))?;

    let top = top_k_by_similarity(&query_embedding, aligned_embeddings, top_k, |i| {
        let Some(pred) = filter else { return true };
        index.chunks.get(i).is_some_and(|c| pred(&c.file_path))
    });

    Ok(top
        .into_iter()
        .filter_map(|(idx, sim)| {
            let chunk = index.chunks.get(idx)?;
            let snippet = chunk.content.lines().take(5).collect::<Vec<_>>().join("\n");
            Some(DenseSearchResult {
                chunk_idx: idx,
                similarity: sim,
                file_path: chunk.file_path.clone(),
                symbol_name: chunk.symbol_name.clone(),
                kind: chunk.kind.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                snippet,
            })
        })
        .collect())
}

/// Min-heap based Top-K selection over a flat embedding buffer.
/// O(n log k) instead of O(n log n) full sort. The filter is applied inline
/// during the scan so only matching chunks are considered — post-filtering
/// cannot drop below `top_k` results regardless of filter selectivity.
///
/// Uses sequential memory access (one dereference) via `FlatEmbeddings::get`,
/// unlike the old `Arc<[Vec<f32>]>` layout which had two-level indirection.
#[cfg(feature = "embeddings")]
fn top_k_by_similarity(
    query: &[f32],
    embeddings: &FlatEmbeddings,
    k: usize,
    filter: impl Fn(usize) -> bool,
) -> Vec<(usize, f32)> {
    use std::cmp::Ordering;
    use std::collections::BinaryHeap;

    #[derive(PartialEq)]
    struct MinEntry(f32, usize);

    impl Eq for MinEntry {}
    impl PartialOrd for MinEntry {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for MinEntry {
        fn cmp(&self, other: &Self) -> Ordering {
            other
                .0
                .partial_cmp(&self.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| self.1.cmp(&other.1))
        }
    }

    let n = embeddings.n_vectors();
    let mut heap: BinaryHeap<MinEntry> = BinaryHeap::with_capacity(k + 1);

    for i in 0..n {
        if !filter(i) {
            continue;
        }
        let emb = embeddings.get(i);
        let sim = crate::core::embeddings::cosine_similarity(query, emb);
        if heap.len() < k {
            heap.push(MinEntry(sim, i));
        } else if let Some(min) = heap.peek()
            && sim > min.0
        {
            heap.pop();
            heap.push(MinEntry(sim, i));
        }
    }

    let mut result: Vec<(usize, f32)> = heap.into_iter().map(|e| (e.1, e.0)).collect();
    result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn set_env(key: &str, value: Option<&str>) -> Option<String> {
        let old = std::env::var(key).ok();
        match value {
            Some(v) => crate::test_env::set_var(key, v),
            None => crate::test_env::remove_var(key),
        }
        old
    }

    fn restore_env(key: &str, old: Option<String>) {
        match old {
            Some(v) => crate::test_env::set_var(key, v),
            None => crate::test_env::remove_var(key),
        }
    }

    #[test]
    fn dense_backend_defaults_to_local() {
        let _g = crate::core::data_dir::test_env_lock();
        let old_backend = set_env("LEANCTX_DENSE_BACKEND", None);
        let old_url = set_env("LEANCTX_QDRANT_URL", None);
        let old_pg = set_env("LEANCTX_PGVECTOR_URL", None);

        let got = DenseBackendKind::try_from_env().unwrap();
        assert_eq!(got, DenseBackendKind::Local);

        restore_env("LEANCTX_DENSE_BACKEND", old_backend);
        restore_env("LEANCTX_QDRANT_URL", old_url);
        restore_env("LEANCTX_PGVECTOR_URL", old_pg);
    }

    #[test]
    fn dense_backend_unknown_value_errors() {
        let _g = crate::core::data_dir::test_env_lock();
        let old_backend = set_env("LEANCTX_DENSE_BACKEND", Some("wat"));
        let old_url = set_env("LEANCTX_QDRANT_URL", None);

        let err = DenseBackendKind::try_from_env().unwrap_err();
        assert!(err.contains("Unknown LEANCTX_DENSE_BACKEND"));

        restore_env("LEANCTX_DENSE_BACKEND", old_backend);
        restore_env("LEANCTX_QDRANT_URL", old_url);
    }
}
