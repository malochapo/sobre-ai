//! `ctx_architecture` — Graph-based architecture analysis tool.
//!
//! Discovers module clusters, dependency layers, entrypoints, cycles,
//! and structural patterns from the Property Graph.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::core::property_graph::CodeGraph;
use crate::core::tokens::count_tokens;
use serde_json::{Value, json};

use crate::tools::graph_meta::{graph_summary, project_meta};
use crate::tools::output_format::{OutputFormat, parse_format};

/// Dispatches the architecture actions reachable from `ctx_graph`: cycles and entrypoints.
pub fn handle(action: &str, root: &str, format: Option<&str>) -> String {
    let fmt = match parse_format(format) {
        Ok(f) => f,
        Err(e) => return e,
    };

    match action {
        "cycles" => handle_cycles(root, fmt),
        "entrypoints" => handle_entrypoints(root, fmt),
        _ => "Unknown action. Use: cycles, entrypoints".to_string(),
    }
}

fn open_graph(root: &str) -> Result<CodeGraph, String> {
    CodeGraph::open(root).map_err(|e| format!("Failed to open graph: {e}"))
}

struct GraphData {
    forward: HashMap<String, Vec<String>>,
    reverse: HashMap<String, Vec<String>>,
    all_files: HashSet<String>,
}

fn ensure_graph_built(root: &str) {
    let Ok(graph) = CodeGraph::open(root) else {
        return;
    };
    if graph.node_count().unwrap_or(0) == 0 {
        drop(graph);
        let result = crate::tools::ctx_impact::handle("build", None, root, None, None);
        tracing::info!(
            "Auto-built graph for architecture: {}",
            &result[..result.len().min(100)]
        );
    }
}

fn load_graph_data(graph: &CodeGraph) -> Result<GraphData, String> {
    let nodes = graph.node_count().map_err(|e| format!("{e}"))?;
    if nodes == 0 {
        return Err(
            "Graph is empty after auto-build. No supported source files found.".to_string(),
        );
    }

    let conn = &graph.connection();
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT p_src.path, p_tgt.path
         FROM edges e
         JOIN nodes n_src ON e.source_id = n_src.id
         JOIN nodes n_tgt ON e.target_id = n_tgt.id
         JOIN paths p_src ON p_src.id = n_src.file_id
         JOIN paths p_tgt ON p_tgt.id = n_tgt.file_id
         WHERE e.kind = 'imports'
           AND n_src.kind = 'file' AND n_tgt.kind = 'file'
           AND n_src.file_id != n_tgt.file_id",
        )
        .map_err(|e| format!("{e}"))?;

    let mut forward: HashMap<String, Vec<String>> = HashMap::new();
    let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_files: HashSet<String> = HashSet::new();

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("{e}"))?;

    for row in rows {
        let (src, tgt) = row.map_err(|e| format!("{e}"))?;
        all_files.insert(src.clone());
        all_files.insert(tgt.clone());
        forward.entry(src.clone()).or_default().push(tgt.clone());
        reverse.entry(tgt).or_default().push(src);
    }

    let mut file_stmt = conn
        .prepare(
            "SELECT DISTINCT p.path
             FROM nodes n JOIN paths p ON p.id = n.file_id
             WHERE n.kind = 'file'",
        )
        .map_err(|e| format!("{e}"))?;
    let file_rows = file_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("{e}"))?;
    for f in file_rows.flatten() {
        all_files.insert(f);
    }

    for deps in forward.values_mut() {
        deps.sort();
        deps.dedup();
    }
    for deps in reverse.values_mut() {
        deps.sort();
        deps.dedup();
    }

    Ok(GraphData {
        forward,
        reverse,
        all_files,
    })
}

fn handle_cycles(root: &str, fmt: OutputFormat) -> String {
    ensure_graph_built(root);
    let graph = match open_graph(root) {
        Ok(g) => g,
        Err(e) => return e,
    };

    let data = match load_graph_data(&graph) {
        Ok(d) => d,
        Err(e) => return e,
    };

    let cycles = find_cycles(&data);
    if cycles.is_empty() {
        return match fmt {
            OutputFormat::Json => {
                let root_path = Path::new(root);
                let v = json!({
                    "schema_version": crate::core::contracts::GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION,
                    "tool": "ctx_graph",
                    "action": "cycles",
                    "project": project_meta(root),
                    "graph": graph_summary(root_path),
                    "graph_meta": crate::core::property_graph::load_meta(root),
                    "cycles_total": 0,
                    "cycles": []
                });
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
            }
            OutputFormat::Text => "No dependency cycles found.".to_string(),
        };
    }

    let total = cycles.len();
    let limit = crate::core::budgets::ARCHITECTURE_CYCLES_LIMIT.max(1);
    let truncated = total > limit;

    match fmt {
        OutputFormat::Json => {
            let root_path = Path::new(root);
            let items: Vec<Value> = cycles
                .iter()
                .take(limit)
                .map(|c| json!({ "path": c, "len": c.len().saturating_sub(1) }))
                .collect();
            let v = json!({
                "schema_version": crate::core::contracts::GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION,
                "tool": "ctx_graph",
                "action": "cycles",
                "project": project_meta(root),
                "graph": graph_summary(root_path),
                "graph_meta": crate::core::property_graph::load_meta(root),
                "cycles_total": total,
                "cycles": items,
                "truncated": truncated
            });
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
        }
        OutputFormat::Text => {
            let mut result = format!("Dependency Cycles ({total}):\n");
            for (i, cycle) in cycles.iter().take(limit).enumerate() {
                result.push_str(&format!("\n#{}: {}\n", i + 1, cycle.join(" -> ")));
            }
            if truncated {
                result.push_str(&format!("\n... +{} more cycles\n", total - limit));
            }

            let tokens = count_tokens(&result);
            format!("{result}[ctx_graph cycles: {tokens} tok]")
        }
    }
}

fn handle_entrypoints(root: &str, fmt: OutputFormat) -> String {
    ensure_graph_built(root);
    let graph = match open_graph(root) {
        Ok(g) => g,
        Err(e) => return e,
    };

    let data = match load_graph_data(&graph) {
        Ok(d) => d,
        Err(e) => return e,
    };

    let entrypoints = find_entrypoints(&data);
    let total = entrypoints.len();
    let limit = crate::core::budgets::ARCHITECTURE_ENTRYPOINTS_LIMIT.max(1);
    let truncated = total > limit;

    match fmt {
        OutputFormat::Json => {
            let root_path = Path::new(root);
            let items: Vec<Value> = entrypoints
                .iter()
                .take(limit)
                .map(|ep| {
                    let imports = data.forward.get(ep).map_or(0, std::vec::Vec::len);
                    json!({ "file": ep, "imports": imports })
                })
                .collect();
            let v = json!({
                "schema_version": crate::core::contracts::GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION,
                "tool": "ctx_graph",
                "action": "entrypoints",
                "project": project_meta(root),
                "graph": graph_summary(root_path),
                "graph_meta": crate::core::property_graph::load_meta(root),
                "entrypoints_total": total,
                "entrypoints": items,
                "truncated": truncated
            });
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
        }
        OutputFormat::Text => {
            let mut result = format!("Entrypoints ({total} — files with no dependents):\n");
            for ep in entrypoints.iter().take(limit) {
                let dep_count = data.forward.get(ep).map_or(0, std::vec::Vec::len);
                result.push_str(&format!("  {ep} (imports {dep_count} files)\n"));
            }
            if truncated {
                result.push_str(&format!("  ... +{} more\n", total - limit));
            }

            let tokens = count_tokens(&result);
            format!("{result}[ctx_graph entrypoints: {tokens} tok]")
        }
    }
}

// ---------------------------------------------------------------------------
// Algorithms
// ---------------------------------------------------------------------------

fn find_entrypoints(data: &GraphData) -> Vec<String> {
    let mut entrypoints: Vec<String> = data
        .all_files
        .iter()
        .filter(|f| !data.reverse.contains_key(*f))
        .cloned()
        .collect();
    entrypoints.sort();
    entrypoints
}

fn find_cycles(data: &GraphData) -> Vec<Vec<String>> {
    let mut cycles: Vec<Vec<String>> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();

    let mut starts: Vec<&String> = data.all_files.iter().collect();
    starts.sort();
    for start in starts {
        if visited.contains(start) {
            continue;
        }

        let mut stack: Vec<String> = Vec::new();
        let mut on_stack: HashSet<String> = HashSet::new();
        dfs_cycles(
            start,
            &data.forward,
            &mut stack,
            &mut on_stack,
            &mut visited,
            &mut cycles,
        );
    }

    cycles.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    cycles.truncate(crate::core::budgets::ARCHITECTURE_CYCLES_LIMIT.max(1));
    cycles
}

fn dfs_cycles(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    stack: &mut Vec<String>,
    on_stack: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    cycles: &mut Vec<Vec<String>>,
) {
    if on_stack.contains(node) {
        let cycle_start = stack.iter().position(|n| n == node).unwrap_or(0);
        let mut cycle: Vec<String> = stack[cycle_start..].to_vec();
        cycle.push(node.to_string());
        cycles.push(cycle);
        return;
    }

    if visited.contains(node) {
        return;
    }

    on_stack.insert(node.to_string());
    stack.push(node.to_string());

    if let Some(deps) = graph.get(node) {
        for dep in deps {
            dfs_cycles(dep, graph, stack, on_stack, visited, cycles);
        }
    }

    stack.pop();
    on_stack.remove(node);
    visited.insert(node.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entrypoints_no_dependents() {
        let mut forward: HashMap<String, Vec<String>> = HashMap::new();
        forward.insert("main.rs".to_string(), vec!["lib.rs".to_string()]);

        let all_files: HashSet<String> = ["main.rs", "lib.rs"]
            .iter()
            .map(std::string::ToString::to_string)
            .collect();

        let data = GraphData {
            forward,
            reverse: {
                let mut r = HashMap::new();
                r.insert("lib.rs".to_string(), vec!["main.rs".to_string()]);
                r
            },
            all_files,
        };

        let eps = find_entrypoints(&data);
        assert_eq!(eps, vec!["main.rs"]);
    }

    #[test]
    fn cycles_detection() {
        let mut forward: HashMap<String, Vec<String>> = HashMap::new();
        forward.insert("a.rs".to_string(), vec!["b.rs".to_string()]);
        forward.insert("b.rs".to_string(), vec!["a.rs".to_string()]);

        let all_files: HashSet<String> = ["a.rs", "b.rs"]
            .iter()
            .map(std::string::ToString::to_string)
            .collect();

        let data = GraphData {
            forward,
            reverse: HashMap::new(),
            all_files,
        };

        let cycles = find_cycles(&data);
        assert!(!cycles.is_empty());
    }

    #[test]
    fn handle_unknown() {
        let result = handle("invalid", "/tmp", None);
        assert!(result.contains("Unknown action"));
    }
}
