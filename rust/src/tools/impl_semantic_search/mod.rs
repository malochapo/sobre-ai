use std::path::Path;

use crate::core::bm25_index::{BM25Index, format_search_results};
#[cfg(feature = "dense")]
use crate::core::embedding_index::EmbeddingIndex;
use crate::tools::CrpMode;

/// The ranking a semantic search uses. Dense and hybrid exist only in builds
/// with the `dense` feature; asking for them elsewhere is an error, never
/// a quiet BM25 answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchMode {
    Bm25,
    #[cfg(feature = "dense")]
    Dense,
    #[cfg(feature = "dense")]
    Hybrid,
}

impl SearchMode {
    /// `None` is BM25.
    pub(crate) fn parse(mode: Option<&str>) -> Result<Self, String> {
        let Some(mode) = mode.map(|m| m.trim().to_ascii_lowercase()) else {
            return Ok(Self::Bm25);
        };
        match mode.as_str() {
            "bm25" => Ok(Self::Bm25),
            #[cfg(feature = "dense")]
            "dense" => Ok(Self::Dense),
            #[cfg(feature = "dense")]
            "hybrid" => Ok(Self::Hybrid),
            #[cfg(not(feature = "dense"))]
            "dense" | "hybrid" => Err(format!(
                "mode={mode} needs local embeddings, which this build does not include \
                 (build with --features dense). Use mode=bm25."
            )),
            _ => Err(format!("unknown mode '{mode}'. Use bm25, dense or hybrid.")),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Bm25 => "bm25",
            #[cfg(feature = "dense")]
            Self::Dense => "dense",
            #[cfg(feature = "dense")]
            Self::Hybrid => "hybrid",
        }
    }
}

/// Performs semantic code search using BM25, dense embeddings, or hybrid ranking.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle(
    query: &str,
    path: &str,
    top_k: usize,
    crp_mode: CrpMode,
    languages: Option<&[String]>,
    path_glob: Option<&str>,
    mode: Option<&str>,
    workspace: Option<bool>,
) -> String {
    let (root_buf, subdir) = match resolve_search_root(path) {
        Ok(v) => v,
        Err(e) => return format!("ERR: {e}"),
    };
    let root = root_buf.as_path();

    let filter = match SearchFilter::new(languages, path_glob) {
        Ok(f) => f.with_subdir(subdir),
        Err(e) => return format!("ERR: invalid filter: {e}"),
    };

    let compact = crp_mode.is_tdd();
    let mode = match SearchMode::parse(mode) {
        Ok(m) => m,
        Err(e) => return format!("ERR: {e}"),
    };

    if workspace.unwrap_or(false) {
        return workspace_search(query, root, top_k, compact, &filter, mode);
    }

    let index = match load_or_refresh_bm25(root) {
        Bm25LoadResult::Ready(idx) => idx,
        Bm25LoadResult::Building => {
            return "BM25 index is being built in the background. \
                    Run sobre_search(action=\"semantic\") again in ~30s, or use action=reindex to wait for completion."
                .to_string();
        }
    };
    if index.doc_count == 0 {
        return "No code files found to index.".to_string();
    }

    match mode {
        SearchMode::Bm25 => {
            let mut results = index.search(query, filtered_candidate_k(top_k, filter.is_active()));
            if filter.is_active() {
                results.retain(|x| filter.matches(&x.file_path));
            }
            results.truncate(top_k);

            let header = if compact {
                format!(
                    "semantic_search(bm25,{top_k}) → {} results, {} chunks indexed\n",
                    results.len(),
                    index.doc_count
                )
            } else {
                format!(
                    "Semantic search (BM25): \"{}\" ({} results from {} indexed chunks)\n",
                    truncate_query(query, 60),
                    results.len(),
                    index.doc_count,
                )
            };
            format!("{header}{}", format_search_results(&results, compact))
        }
        #[cfg(feature = "dense")]
        SearchMode::Dense | SearchMode::Hybrid => {
            let ranked = if mode == SearchMode::Dense {
                dense_ranked(query, root, &index, top_k, &filter)
            } else {
                let cfg = crate::core::hybrid_search::HybridConfig::default();
                hybrid_ranked(query, root, &index, top_k, &filter, &cfg)
            };
            let out = match ranked {
                Ok(r) => render_ranked(mode, query, &index, top_k, compact, &r),
                Err(e) => format!("ERR: {e}"),
            };
            shrink_resident_after_embedding(root, index);
            out
        }
    }
}

/// Reclaim the RAM held by full chunk bodies in the resident BM25 cache once the
/// dense/hybrid embedding pass has consumed and persisted them. Drops this
/// handler's `Arc` clone first so the cache becomes the sole owner and the trim
/// is zero-copy (see `bm25_cache::shrink_resident_to_snippet`).
///
/// `keep_lines = 5` matches the snippet window used everywhere results are
/// rendered (`bm25_index::search`, `dense_backend`, `hybrid_search`). Only fires
/// when embeddings are actually built (feature-gated); a BM25-only fallback build
/// must keep full bodies for a later real embedding pass.
#[cfg(feature = "dense")]
fn shrink_resident_after_embedding(root: &Path, index: std::sync::Arc<BM25Index>) {
    {
        // Release our clone so the cache is the sole Arc owner; otherwise the
        // in-place trim is skipped and retried on the next search.
        drop(index);
        if let Some(cache) = get_thread_cache() {
            let freed = crate::core::bm25_cache::shrink_resident_to_snippet(&cache, root, 5);
            if freed > 0 {
                tracing::info!(
                    "[bm25_cache] reclaimed ~{:.1}MB of resident chunk bodies post-embedding",
                    freed as f64 / 1_048_576.0
                );
            }
        }
    }
}

/// Rebuilds the BM25 search index for the given directory from scratch.
#[must_use]
pub(crate) fn handle_reindex(path: &str) -> String {
    // Promote to the project root so the rebuilt index lands in the same
    // namespace the search path resolves to (#948) — reindexing a subdirectory
    // would otherwise build an index the search can never find.
    let (root_buf, _subdir) = match resolve_search_root(path) {
        Ok(v) => v,
        Err(e) => return format!("ERR: {e}"),
    };
    let root = root_buf.as_path();

    let idx = BM25Index::build_from_directory(root);
    let files = idx.files.len();
    let chunks = idx.doc_count;
    let _ = idx.save(root);
    // The next search must use this index, not the one resident before it.
    bm25_store::store_in_thread_cache(root, &std::sync::Arc::new(idx));

    format!(
        "Reindexed {}: {files} files, {chunks} chunks",
        root.display()
    )
}

/// Find chunks semantically related to a given file location.
///
/// Marchionini (2006): Exploratory search navigates from known points.
/// This enables "show me similar code" workflows.
pub(crate) fn handle_find_related(
    file_path: &str,
    line: usize,
    project_root: &str,
    top_k: usize,
    crp_mode: CrpMode,
) -> String {
    let (root_buf, _subdir) = match resolve_search_root(project_root) {
        Ok(v) => v,
        Err(e) => return format!("ERR: {e}"),
    };
    let root = root_buf.as_path();

    let index = BM25Index::load_or_build(root);
    if index.doc_count == 0 {
        return "ERR: empty index. Try action=reindex first.".to_string();
    }

    let source_chunk = index
        .chunks
        .iter()
        .find(|c| c.file_path == file_path && c.start_line <= line && c.end_line >= line);

    let Some(source_chunk) = source_chunk else {
        return format!(
            "ERR: no indexed chunk found at {file_path}:{line}. Try action=reindex first."
        );
    };

    let query_text = source_chunk.content.clone();
    let source_file = source_chunk.file_path.clone();
    let source_start = source_chunk.start_line;

    let compact = crp_mode != CrpMode::Off;
    let (engine, results) = related_chunks(&query_text, root, &index, top_k + 5, compact);

    let lines: Vec<String> = results
        .into_iter()
        .filter(|l| !l.contains(&format!("{source_file}:{source_start}-")))
        .take(top_k)
        .collect();

    let header = if compact {
        format!(
            "find_related({file_path}:{line},{engine}) → {} results\n",
            lines.len()
        )
    } else {
        format!("Find related to {file_path}:{line} ({engine} search on the chunk's text)\n")
    };
    format!("{header}{}", lines.join(""))
}

/// Rank chunks against `text`, one line each. Hybrid when this build has
/// embeddings, BM25 otherwise; returns which one ran.
fn related_chunks(
    text: &str,
    root: &Path,
    index: &BM25Index,
    top_k: usize,
    compact: bool,
) -> (&'static str, Vec<String>) {
    let Ok(filter) = SearchFilter::new(None, None) else {
        return ("bm25", Vec::new());
    };
    #[cfg(feature = "dense")]
    {
        let cfg = crate::core::hybrid_search::HybridConfig::default();
        match hybrid_ranked(text, root, index, top_k, &filter, &cfg) {
            Ok(r) => {
                let engine = if r.degraded.is_some() {
                    "bm25+graph, dense index not built"
                } else {
                    "hybrid"
                };
                let body = crate::core::hybrid_search::format_hybrid_results(&r.results, compact);
                (engine, body.lines().map(|l| format!("{l}\n")).collect())
            }
            Err(e) => ("hybrid", vec![format!("ERR: {e}\n")]),
        }
    }
    #[cfg(not(feature = "dense"))]
    {
        let _ = root;
        let mut results = index.search(text, filtered_candidate_k(top_k, filter.is_active()));
        results.truncate(top_k);
        let body = format_search_results(&results, compact);
        ("bm25", body.lines().map(|l| format!("{l}\n")).collect())
    }
}

fn truncate_query(q: &str, max: usize) -> &str {
    if q.len() <= max {
        return q;
    }
    match q.char_indices().nth(max) {
        Some((byte_idx, _)) => &q[..byte_idx],
        None => q,
    }
}

mod bm25_store;
#[cfg(feature = "dense")]
mod dense;
pub(crate) mod multi_root;
mod scope;

pub(crate) use bm25_store::*;
pub(crate) use bm25_store::{get_thread_cache, set_thread_cache};
#[cfg(feature = "dense")]
pub(crate) use dense::*;
pub(crate) use multi_root::*;
pub(crate) use scope::*;

#[cfg(test)]
mod tests;
