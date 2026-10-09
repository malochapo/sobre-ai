//! Related-file hints from the Property Graph.
//!
//! `sobre_read` appends a one-line hint of the nearest neighbours; the session
//! compactor uses the CSV form. A budgeted `GraphContext` block also lived
//! here, gated on a profile output-hint no shipped profile ever set.

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
