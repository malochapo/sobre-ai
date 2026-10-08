//! `sobre_graph action=impact|changes`: what breaks when a file, a type or the
//! uncommitted changes move. A breadth-first walk of reverse dependency edges
//! in the SQLite property graph.

use crate::core::property_graph::{CodeGraph, ImpactResult};
use crate::core::tokens::count_tokens;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::process::Stdio;

use crate::tools::graph_meta::{graph_summary, project_meta};
use crate::tools::output_format::{OutputFormat, parse_format};

/// Whether the graph was built from the current checkout: `stale` when the
/// git `HEAD` or the dirty state differs from the build's, `fresh` otherwise.
pub(crate) fn freshness(root: &str) -> &'static str {
    let root_path = std::path::Path::new(root);
    let Some(meta) = crate::core::property_graph::load_meta(root) else {
        return "unknown";
    };
    let head = crate::core::git_util::git_out(root_path, &["rev-parse", "--short", "HEAD"]);
    let head_moved =
        matches!((meta.git_head.as_ref(), head.as_ref()), (Some(a), Some(b)) if a != b);
    let dirty_changed = meta
        .git_dirty
        .is_some_and(|d| d != crate::core::git_util::git_dirty(root_path));
    if head_moved || dirty_changed {
        "stale"
    } else {
        "fresh"
    }
}

/// Hops followed from the target when `depth` is not given.
pub const DEFAULT_DEPTH: usize = 5;

/// `action=impact`: the files that depend on `path`, a file or a type name.
pub fn impact(
    path: Option<&str>,
    root: &str,
    depth: Option<usize>,
    format: Option<&str>,
) -> String {
    match parse_format(format) {
        Ok(fmt) => handle_analyze(path, root, depth.unwrap_or(DEFAULT_DEPTH), fmt),
        Err(e) => e,
    }
}

/// `action=changes`: the files that depend on what changed since `HEAD`
/// (working tree, staged and untracked files).
pub fn changes(root: &str, depth: Option<usize>, format: Option<&str>) -> String {
    match parse_format(format) {
        Ok(fmt) => handle_diff(root, depth.unwrap_or(DEFAULT_DEPTH), fmt),
        Err(e) => e,
    }
}

/// The files affected by a change to `target` (a file or a type name), for
/// callers that need the list rather than the rendered text. `None` when the
/// target is neither an indexed file nor a known symbol.
pub(crate) fn affected_files(target: &str, root: &str, depth: usize) -> Option<Vec<String>> {
    let graph = open_graph_fresh(root).ok()?;
    let rel_target = graph_target_key(target, root);
    if graph.get_node_by_path(&rel_target).ok().flatten().is_some() {
        let mut files = graph
            .impact_analysis(&rel_target, depth)
            .ok()?
            .affected_files;
        files.sort();
        return Some(files);
    }
    let symbol = symbol_query_name(target);
    let def_files = graph.resolve_symbol_def_files(&symbol).ok()?;
    if symbol.is_empty() || def_files.is_empty() {
        return None;
    }
    let mut affected: BTreeSet<String> = BTreeSet::new();
    for f in &def_files {
        if let Ok(r) = graph.impact_analysis(f, depth) {
            affected.extend(r.affected_files);
        }
    }
    for f in &def_files {
        affected.remove(f);
    }
    Some(affected.into_iter().collect())
}

fn open_graph(root: &str) -> Result<CodeGraph, String> {
    CodeGraph::open(root).map_err(|e| format!("Failed to open graph: {e}"))
}

/// Open the property graph for a *query*, rebuilding first when it cannot be
/// trusted: either empty (never built) or produced by an engine older than
/// [`crate::core::property_graph::GRAPH_ENGINE_VERSION`]. The rebuild is
/// one-shot and idempotent: a fresh build stamps the current engine version,
/// so a healthy graph is returned without rebuilding.
fn open_graph_fresh(root: &str) -> Result<CodeGraph, String> {
    let graph = open_graph(root)?;
    let empty = graph.node_count().unwrap_or(0) == 0;
    let outdated = !empty && crate::core::property_graph::engine_outdated(root);
    if empty || outdated {
        drop(graph);
        crate::core::graph_index::scan(root);
        tracing::info!(
            "Rebuilt property graph before impact query ({})",
            if empty { "empty" } else { "engine outdated" },
        );
        return open_graph(root);
    }
    Ok(graph)
}

fn handle_analyze(path: Option<&str>, root: &str, max_depth: usize, fmt: OutputFormat) -> String {
    let Some(target) = path else {
        return "ERROR: path is required for action=impact".to_string();
    };

    let graph = match open_graph_fresh(root) {
        Ok(g) => g,
        Err(e) => return e,
    };

    if graph.node_count().unwrap_or(0) == 0 {
        return "Graph is empty after auto-build. No supported source files found.".to_string();
    }

    let rel_target = graph_target_key(target, root);

    // 1) Direct file-node match — the documented contract (a file path).
    if graph.get_node_by_path(&rel_target).ok().flatten().is_some() {
        let impact = match graph.impact_analysis(&rel_target, max_depth) {
            Ok(r) => r,
            Err(e) => return format!("Impact analysis failed: {e}"),
        };
        return format_impact(&impact, &rel_target, root, fmt);
    }

    // 2) Symbol-name fallback: callers — and LLMs — routinely ask for
    //    the impact of a *class/type* by name (`sobre_graph impact ArcPoint`)
    //    rather than its file path. Resolve the bare name to the file(s) that
    //    define it and report their combined blast radius, instead of the
    //    misleading "leaf node" answer a non-file target produced before.
    let symbol = symbol_query_name(target);
    if !symbol.is_empty()
        && let Ok(def_files) = graph.resolve_symbol_def_files(&symbol)
        && !def_files.is_empty()
    {
        return analyze_symbol(&graph, &symbol, &def_files, root, max_depth, fmt);
    }

    // 3) Neither a file nor a known symbol: an actionable diagnostic beats a
    //    false "no impact".
    analyze_unresolved(&graph, target, &rel_target, root, fmt)
}

/// Reduce a user-supplied target to a bare symbol name for the fallback:
/// drop any directory prefix and a single trailing source extension, so
/// `Models/ArcPoint.cs`, `ArcPoint.cs` and `ArcPoint` all query `ArcPoint`.
/// Returns an empty string for inputs that cannot name a single symbol
/// (namespace separators, generics, globs, whitespace) — those would only
/// produce bogus matches.
fn symbol_query_name(target: &str) -> String {
    let base = target.rsplit(['/', '\\']).next().unwrap_or(target).trim();
    let stem = base
        .rsplit_once('.')
        .filter(|(_, ext)| crate::core::language_capabilities::is_indexable_ext(ext))
        .map_or(base, |(s, _)| s);
    if stem.is_empty()
        || stem.contains(|c: char| {
            c.is_whitespace() || matches!(c, '.' | ':' | '*' | '<' | '>' | '(' | ')' | '/' | '\\')
        })
    {
        return String::new();
    }
    stem.to_string()
}

/// Combined blast radius of every file that defines `symbol` (the
/// symbol-name fallback). The defining files are what changes, so they are
/// excluded from the affected set; the resolved files are surfaced so the
/// answer stays transparent. Output is sorted + capped for determinism.
fn analyze_symbol(
    graph: &CodeGraph,
    symbol: &str,
    def_files: &[String],
    root: &str,
    max_depth: usize,
    fmt: OutputFormat,
) -> String {
    let mut affected: BTreeSet<String> = BTreeSet::new();
    let mut max_depth_reached = 0usize;
    let mut edges_traversed = 0usize;
    for f in def_files {
        if let Ok(r) = graph.impact_analysis(f, max_depth) {
            max_depth_reached = max_depth_reached.max(r.max_depth_reached);
            edges_traversed += r.edges_traversed;
            affected.extend(r.affected_files);
        }
    }
    // The definers are the thing being changed, not impacted by it.
    for f in def_files {
        affected.remove(f);
    }

    let mut sorted: Vec<String> = affected.into_iter().collect();
    let total = sorted.len();
    let limit = crate::core::budgets::IMPACT_AFFECTED_FILES_LIMIT.max(1);
    let truncated = total > limit;
    if truncated {
        sorted.truncate(limit);
    }

    match fmt {
        OutputFormat::Json => {
            let v = json!({
                "schema_version": crate::core::contracts::GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION,
                "tool": "sobre_graph",
                "action": "impact",
                "project": project_meta(root),
                "graph": graph_summary(root),
                "graph_meta": crate::core::property_graph::load_meta(root),
                "target": symbol,
                "resolved_from": "symbol",
                "defined_in": def_files,
                "max_depth_reached": max_depth_reached,
                "edges_traversed": edges_traversed,
                "affected_files_total": total,
                "affected_files": sorted,
                "truncated": truncated
            });
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
        }
        OutputFormat::Text => {
            let defined = def_files.join(", ");
            if total == 0 {
                let result = format!(
                    "No files depend on {symbol} (defined in {defined}); it is a leaf in the dependency graph."
                );
                let tokens = count_tokens(&result);
                return format!("{result}\n[sobre_graph impact: {tokens} tok]");
            }
            let mut result = format!(
                "Impact of changing {symbol} (defined in {defined}): {total} affected files \
                 (depth: {max_depth_reached}, edges traversed: {edges_traversed})\n"
            );
            for file in &sorted {
                result.push_str(&format!("  {file}\n"));
            }
            if truncated {
                result.push_str(&format!("  ... +{} more\n", total - limit));
            }
            let tokens = count_tokens(&result);
            format!("{result}[sobre_graph impact: {tokens} tok]")
        }
    }
}

/// Diagnostic for an `impact` target that matched neither a file node nor a
/// symbol. Replaces the old silent "leaf node" answer — indistinguishable from
/// a real leaf — with the indexed counts and a concrete next step.
fn analyze_unresolved(
    graph: &CodeGraph,
    target: &str,
    rel_target: &str,
    root: &str,
    fmt: OutputFormat,
) -> String {
    let files = graph.file_node_count().unwrap_or(0);
    let symbols = graph.symbol_count().unwrap_or(0);
    match fmt {
        OutputFormat::Json => {
            let v = json!({
                "tool": "sobre_graph",
                "action": "impact",
                "project": project_meta(root),
                "graph": graph_summary(root),
                "target": target,
                "resolved": false,
                "indexed_files": files,
                "indexed_symbols": symbols,
                "hint": "Target is neither an indexed file path nor a known symbol. Pass a path relative to the project root, or rebuild with sobre_graph action='build'."
            });
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
        }
        OutputFormat::Text => {
            let result = format!(
                "'{target}' is not a known file or symbol in the graph \
                 ({files} files, {symbols} symbols indexed).\n  \
                 - As a file: pass a path relative to the project root (looked up '{rel_target}').\n  \
                 - As a class/type: check the spelling, or run sobre_graph action='build' to (re)index."
            );
            let tokens = count_tokens(&result);
            format!("{result}\n[sobre_graph impact: {tokens} tok]")
        }
    }
}

fn format_impact(impact: &ImpactResult, target: &str, root: &str, fmt: OutputFormat) -> String {
    let mut sorted = impact.affected_files.clone();
    sorted.sort();

    let total = sorted.len();
    let limit = crate::core::budgets::IMPACT_AFFECTED_FILES_LIMIT.max(1);
    let truncated = total > limit;
    if truncated {
        sorted.truncate(limit);
    }

    match fmt {
        OutputFormat::Json => {
            let v = json!({
                "schema_version": crate::core::contracts::GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION,
                "tool": "sobre_graph",
                "action": "impact",
                "project": project_meta(root),
                "graph": graph_summary(root),
                "graph_meta": crate::core::property_graph::load_meta(root),
                "target": target,
                "max_depth_reached": impact.max_depth_reached,
                "edges_traversed": impact.edges_traversed,
                "affected_files_total": total,
                "affected_files": sorted,
                "truncated": truncated
            });
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
        }
        OutputFormat::Text => {
            if total == 0 {
                let result =
                    format!("No files depend on {target} (leaf node in the dependency graph).");
                let tokens = count_tokens(&result);
                return format!("{result}\n[sobre_graph impact: {tokens} tok]");
            }

            let mut result = format!(
                "Impact of changing {target}: {total} affected files (depth: {}, edges traversed: {})\n",
                impact.max_depth_reached, impact.edges_traversed
            );

            for file in &sorted {
                result.push_str(&format!("  {file}\n"));
            }
            if truncated {
                result.push_str(&format!("  ... +{} more\n", total - limit));
            }

            let tokens = count_tokens(&result);
            format!("{result}[sobre_graph impact: {tokens} tok]")
        }
    }
}

fn handle_diff(root: &str, max_depth: usize, fmt: OutputFormat) -> String {
    let changed = git_changed_files(root);
    if changed.is_empty() {
        return match fmt {
            OutputFormat::Json => {
                let v = json!({
                    "tool": "sobre_graph",
                    "action": "changes",
                    "changed_files": [],
                    "blast_radius": [],
                    "total_affected": 0
                });
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
            }
            OutputFormat::Text => "No uncommitted changes found.".to_string(),
        };
    }

    let graph = match open_graph_fresh(root) {
        Ok(g) => g,
        Err(e) => return e,
    };

    compute_diff_impact(&graph, &changed, root, max_depth, fmt)
}

/// Files changed since `HEAD` (working tree, staged and untracked), relative to
/// `root` and limited to it, like the graph's own paths.
pub(crate) fn git_changed_files(root: &str) -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", "--relative", "HEAD"])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let mut files: BTreeSet<String> = BTreeSet::new();

    if let Ok(o) = output
        && o.status.success()
    {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                files.insert(trimmed.to_string());
            }
        }
    }

    let staged = std::process::Command::new("git")
        .args(["diff", "--name-only", "--relative", "--cached"])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    if let Ok(o) = staged
        && o.status.success()
    {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                files.insert(trimmed.to_string());
            }
        }
    }

    let untracked = std::process::Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    if let Ok(o) = untracked
        && o.status.success()
    {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                files.insert(trimmed.to_string());
            }
        }
    }

    files.into_iter().collect()
}

fn compute_diff_impact(
    graph: &CodeGraph,
    changed: &[String],
    root: &str,
    max_depth: usize,
    fmt: OutputFormat,
) -> String {
    let mut all_affected: BTreeSet<String> = BTreeSet::new();
    let mut per_file: Vec<(String, Vec<String>)> = Vec::new();

    for file in changed {
        let rel = graph_target_key(file, root);
        if let Ok(impact) = graph.impact_analysis(&rel, max_depth) {
            let mut affected: Vec<String> = impact
                .affected_files
                .into_iter()
                .filter(|f| !changed.contains(f))
                .collect();
            affected.sort();
            for a in &affected {
                all_affected.insert(a.clone());
            }
            if !affected.is_empty() {
                per_file.push((rel, affected));
            }
        }
    }

    match fmt {
        OutputFormat::Json => {
            let items: Vec<Value> = per_file
                .iter()
                .map(|(file, affected)| {
                    json!({
                        "changed_file": file,
                        "affected": affected,
                        "count": affected.len()
                    })
                })
                .collect();
            let v = json!({
                "tool": "sobre_graph",
                "action": "changes",
                "changed_files": changed,
                "blast_radius": items,
                "total_affected": all_affected.len()
            });
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
        }
        OutputFormat::Text => {
            let mut result = format!(
                "Impact of uncommitted changes ({} changed files, {} affected)\n\n",
                changed.len(),
                all_affected.len()
            );
            result.push_str("Changed files:\n");
            for f in changed.iter().take(30) {
                result.push_str(&format!("  {f}\n"));
            }

            if !per_file.is_empty() {
                result.push_str("\nBlast radius:\n");
                for (file, affected) in per_file.iter().take(15) {
                    result.push_str(&format!("  {file} -> {} affected\n", affected.len()));
                    for a in affected.iter().take(10) {
                        result.push_str(&format!("    {a}\n"));
                    }
                    if affected.len() > 10 {
                        result.push_str(&format!("    ... +{} more\n", affected.len() - 10));
                    }
                }
            }

            let tokens = count_tokens(&result);
            format!("{result}\n[sobre_graph changes: {tokens} tok]")
        }
    }
}

fn graph_target_key(path: &str, root: &str) -> String {
    let rel = crate::core::index_paths::graph_relative_key(path, root);
    let rel_key = crate::core::index_paths::graph_match_key(&rel);
    if rel_key.is_empty() {
        crate::core::index_paths::graph_match_key(path)
    } else {
        rel_key
    }
}

#[cfg(test)]
#[path = "sobre_graph_impact_tests.rs"]
mod tests;
