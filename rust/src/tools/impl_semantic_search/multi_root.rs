//! Workspace search across roots + RRF fusion, graph-signal ranks, and the
//! SPLADE boost of the hybrid pipeline.

use std::path::Path;

use crate::core::bm25_index::BM25Index;
use crate::core::hybrid_search::{HybridResult, format_hybrid_results};

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) const WORKSPACE_RRF_K: f64 = 60.0;

/// The roots a workspace search spans: the project, plus every `search_roots`
/// directory from the config. Missing directories are skipped.
pub(crate) fn workspace_roots(root: &Path) -> Vec<std::path::PathBuf> {
    let cfg = crate::core::config::Config::load();
    let mut roots = vec![root.to_path_buf()];
    for entry in &cfg.search_roots {
        let p = crate::core::pathutil::canonicalize_or_self(
            &crate::core::pathutil::expand_user_path(entry),
        );
        if p.is_dir() {
            roots.push(p);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Search the project and every configured `search_roots` directory, then
/// fuse the per-root rankings by RRF. A root whose search fails is reported in
/// the output, never silently replaced by another engine.
pub(crate) fn workspace_search(
    query: &str,
    root: &Path,
    top_k: usize,
    compact: bool,
    filter: &SearchFilter,
    mode: SearchMode,
) -> String {
    let mut warnings: Vec<String> = Vec::new();
    let roots = workspace_roots(root);
    let mut per_project: Vec<(String, Vec<HybridResult>)> = Vec::new();

    for r in &roots {
        let label = label_for_root(r);
        let index = BM25Index::load_or_build(r);
        if index.doc_count == 0 {
            continue;
        }
        let mut results = match root_results(mode, query, r, &index, top_k, filter) {
            Ok((results, note)) => {
                if let Some(note) = note {
                    warnings.push(format!("[{label}] {note}"));
                }
                results
            }
            Err(e) => {
                warnings.push(format!("[{label}] {} search failed: {e}", mode.as_str()));
                continue;
            }
        };
        for res in &mut results {
            res.file_path = format!("[project:{label}] {}", res.file_path);
        }
        per_project.push((label, results));
    }

    let mut fused: Vec<HybridResult> = if per_project.len() <= 1 {
        per_project
            .into_iter()
            .next()
            .map(|(_, v)| v)
            .unwrap_or_default()
    } else {
        rrf_merge_hybrid(per_project, top_k)
    };
    fused.truncate(top_k);

    let mode = mode.as_str();
    let mut out = if compact {
        format!(
            "semantic_search(workspace,{mode},{top_k}) → {} results, projects={}\n",
            fused.len(),
            roots.len()
        )
    } else {
        format!(
            "Workspace semantic search ({mode}): \"{}\" ({} results from {} projects)\n",
            truncate_query(query, 60),
            fused.len(),
            roots.len()
        )
    };
    out.push_str(&format_hybrid_results(&fused, compact));
    for w in &warnings {
        out.push_str(&format!("\n{w}"));
    }
    out
}

/// One root's ranking in `mode`, plus a note when hybrid ran without its
/// dense signal. Only the dense arms read `root` or can fail.
#[cfg_attr(
    not(feature = "dense"),
    allow(unused_variables, clippy::unnecessary_wraps)
)]
fn root_results(
    mode: SearchMode,
    query: &str,
    root: &Path,
    index: &BM25Index,
    top_k: usize,
    filter: &SearchFilter,
) -> Result<(Vec<HybridResult>, Option<String>), String> {
    match mode {
        SearchMode::Bm25 => {
            let mut bm25 = index.search(query, filtered_candidate_k(top_k, filter.is_active()));
            if filter.is_active() {
                bm25.retain(|x| filter.matches(&x.file_path));
            }
            bm25.truncate(top_k);
            Ok((
                bm25.into_iter()
                    .map(HybridResult::from_bm25_public)
                    .collect(),
                None,
            ))
        }
        #[cfg(feature = "dense")]
        SearchMode::Dense => {
            dense_ranked(query, root, index, top_k, filter).map(|r| (r.results, None))
        }
        #[cfg(feature = "dense")]
        SearchMode::Hybrid => {
            let cfg = crate::core::hybrid_search::HybridConfig::default();
            hybrid_ranked(query, root, index, top_k, filter, &cfg).map(|r| (r.results, r.degraded))
        }
    }
}

pub(crate) fn rrf_merge_hybrid(
    lists: Vec<(String, Vec<HybridResult>)>,
    top_k: usize,
) -> Vec<HybridResult> {
    use std::collections::HashMap;

    let mut acc: HashMap<String, (HybridResult, f64)> = HashMap::new();
    for (label, results) in lists {
        for (rank, r) in results.into_iter().enumerate() {
            let key = format!(
                "{label}|{}|{}|{}|{}",
                r.file_path, r.symbol_name, r.start_line, r.end_line
            );
            let rrf = 1.0 / (WORKSPACE_RRF_K + (rank as f64) + 1.0);
            acc.entry(key)
                .and_modify(|(_, s)| *s += rrf)
                .or_insert((r, rrf));
        }
    }

    let mut out: Vec<HybridResult> = acc
        .into_values()
        .map(|(mut r, s)| {
            r.rrf_score = s;
            r
        })
        .collect();
    out.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.symbol_name.cmp(&b.symbol_name))
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.end_line.cmp(&b.end_line))
    });
    out.truncate(top_k);
    out
}
/// Boost existing hybrid results with SPLADE expansion scores.
#[cfg(feature = "dense")]
pub(crate) fn boost_with_splade(
    results: &mut [HybridResult],
    splade: &[crate::core::splade_retrieval::SpladeResult],
    weight: f64,
) {
    use std::collections::HashMap;
    let rrf_k = 60.0_f64;

    let boosts: HashMap<&str, f64> = splade
        .iter()
        .enumerate()
        .map(|(rank, sr)| (sr.file_path.as_str(), weight / (rrf_k + rank as f64 + 1.0)))
        .collect();

    for r in results.iter_mut() {
        if let Some(&boost) = boosts.get(r.file_path.as_str()) {
            r.rrf_score += boost;
        }
    }

    results.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

pub(crate) fn label_for_root(root: &Path) -> String {
    root.file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| root.to_string_lossy().to_string())
}
