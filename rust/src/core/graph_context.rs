//! Related-file hints from the Property Graph.
//!
//! `sobre_read` appends a one-line hint of the nearest neighbours; the session
//! compactor uses the CSV form. A budgeted `GraphContext` block also lived
//! here, gated on a profile output-hint no shipped profile ever set.

#[cfg(feature = "dense")]
use std::collections::{HashMap, HashSet};

use super::graph_provider;

fn related_files_scored_for_path(
    file_path: &str,
    project_root: &str,
    limit: usize,
) -> Option<Vec<(String, f64)>> {
    let provider = graph_provider::open_best_effort(project_root)?;
    let rel_path = file_path
        .strip_prefix(project_root)
        .unwrap_or(file_path)
        .trim_start_matches('/');
    let scored = provider.related_files_scored(rel_path, limit);
    if scored.is_empty() {
        return None;
    }
    Some(scored)
}

/// Comma-separated repo-relative paths for dependency-cluster hints (e.g. CCP XML).
pub fn build_related_paths_csv(
    file_path: &str,
    project_root: &str,
    limit: usize,
) -> Option<String> {
    let scored = related_files_scored_for_path(file_path, project_root, limit)?;
    Some(
        scored
            .into_iter()
            .map(|(path, _)| path)
            .collect::<Vec<_>>()
            .join(","),
    )
}

#[cfg(feature = "dense")]
/// Repo-relative paths (same style as BM25 chunks / property graph) ranked for RRF graph proximity.
///
/// `recent_repo_paths` should be ordered **most recently touched first**. Neighbors from earlier
/// seeds are ranked before those from later seeds; within a seed, graph `related_files_scored` order is kept.
pub fn graph_neighbor_ranks_for_recent_files(
    project_root: &str,
    recent_repo_paths: &[String],
    per_seed_limit: usize,
    max_ranked: usize,
) -> Option<HashMap<String, usize>> {
    // Two neighbour sources, blended per seed:
    //   1. Traversal (co-access) — files the agent actually opened *together*
    //      (behavioural, the strongest "what next" signal). Listed first so it
    //      gets the best RRF rank. Works even without a static graph (#289).
    //   2. Static graph — import/call/type proximity.
    let coaccess = crate::core::cooccurrence::load(project_root);
    let open = graph_provider::open_best_effort(project_root);

    let mut seen = HashSet::<String>::new();
    let mut ranked: Vec<String> = Vec::new();

    'seeds: for seed in recent_repo_paths {
        let rel_path = normalize_repo_rel_path(seed, project_root);
        if rel_path.is_empty() {
            continue;
        }

        for (path, _) in coaccess.related(&rel_path, per_seed_limit) {
            if seen.insert(path.clone()) {
                ranked.push(path);
                if ranked.len() >= max_ranked {
                    break 'seeds;
                }
            }
        }

        if let Some(open) = &open {
            for (path, _) in open.related_files_scored(&rel_path, per_seed_limit) {
                if seen.insert(path.clone()) {
                    ranked.push(path);
                    if ranked.len() >= max_ranked {
                        break 'seeds;
                    }
                }
            }
        }
    }

    if ranked.is_empty() {
        None
    } else {
        Some(
            ranked
                .into_iter()
                .enumerate()
                .map(|(i, p)| (p, i))
                .collect(),
        )
    }
}

#[cfg(feature = "dense")]
fn normalize_repo_rel_path(path: &str, project_root: &str) -> String {
    let p = path.replace('\\', "/");
    let root = project_root.trim_end_matches('/').replace('\\', "/");
    let prefix = format!("{root}/");
    if let Some(rest) = p.strip_prefix(&prefix) {
        return rest.to_string();
    }
    p.trim_start_matches('/').to_string()
}
