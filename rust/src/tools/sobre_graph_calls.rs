//! The call actions of `sobre_graph` — `callers`, `callees` and `trace` — answered
//! from the function-level call graph ([`crate::core::call_graph`]).

use crate::core::call_graph::{CallGraph, CallGraphInputs};
use crate::core::index_paths;

const MAX_BFS_DEPTH: usize = 5;

/// The project's call graph: the persisted graph with every file whose content
/// changed re-analyzed, saved back before it is returned.
pub fn load(project_root: &str) -> CallGraph {
    let inputs = CallGraphInputs::open(project_root);
    let graph = CallGraph::load_or_build(project_root, &inputs);
    let _ = graph.save();
    graph
}

/// `sobre_graph action=callers|callees`: the callers (`callers`) or callees of
/// `symbol` up to `depth` hops (1-5), restricted to call sites in the file
/// `scope` when given.
pub fn callers_or_callees(
    symbol: Option<&str>,
    callers: bool,
    scope: Option<&str>,
    depth: usize,
    project_root: &str,
) -> String {
    let Some(symbol) = symbol else {
        let action = if callers { "callers" } else { "callees" };
        return format!("ERROR: symbol is required for action={action}");
    };
    let graph = load(project_root);
    let filter = scope.map(|f| graph_file_filter(f, project_root));
    let depth = depth.clamp(1, MAX_BFS_DEPTH);
    let body = if depth == 1 {
        direct(&graph, symbol, callers, filter.as_deref())
    } else {
        transitive(&graph, symbol, callers, depth, filter.as_deref())
    };
    let note = if callers || filter.is_some() {
        None
    } else {
        ambiguity_note(&graph, symbol)
    };
    with_handle_hint(match note {
        Some(note) => format!("{note}\n{body}"),
        None => body,
    })
}

/// When functions named `symbol` call out from more than one file, their
/// callees are merged: say so, and name the files to scope with `path=`.
fn ambiguity_note(graph: &CallGraph, symbol: &str) -> Option<String> {
    let mut files: Vec<&str> = graph
        .callees_of(symbol)
        .into_iter()
        .map(|e| e.caller_file.as_str())
        .collect();
    files.sort_unstable();
    files.dedup();
    (files.len() > 1).then(|| {
        format!(
            "[sobre: '{symbol}' names functions in {} files ({}): their callees are merged; \
             pass path= to pick one]",
            files.len(),
            files.join(", ")
        )
    })
}

/// `sobre_graph action=trace`: the shortest call chain from `symbol` to `to`.
pub fn trace(symbol: Option<&str>, to: Option<&str>, project_root: &str) -> String {
    let Some(from) = symbol else {
        return "ERROR: symbol is required for action=trace".to_string();
    };
    let Some(to) = to else {
        return "ERROR: to is required for action=trace".to_string();
    };
    with_handle_hint(call_path(&load(project_root), from, to))
}

/// Every call edge into (`callers`) or out of `symbol`, one line per call site.
fn direct(graph: &CallGraph, symbol: &str, callers: bool, filter: Option<&str>) -> String {
    let mut edges = if callers {
        graph.callers_of(symbol)
    } else {
        graph.callees_of(symbol)
    };
    if let Some(f) = filter {
        edges.retain(|e| index_paths::graph_match_key(&e.caller_file).contains(f));
    }
    let noun = if callers { "caller" } else { "callee" };

    if edges.is_empty() {
        return format!(
            "No {noun}s found for '{symbol}' ({} edges in graph)",
            graph.edges.len()
        );
    }

    let mut out = format!("{} {noun}(s) of '{symbol}':\n", edges.len());
    for edge in &edges {
        if callers {
            out.push_str(&format!(
                "  {} → {}  (L{})\n",
                edge.caller_file, edge.caller_symbol, edge.caller_line
            ));
        } else {
            out.push_str(&format!(
                "  → {}  ({}:L{})\n",
                edge.callee_name, edge.caller_file, edge.caller_line
            ));
        }
    }
    out
}

/// The callers (or callees) of `symbol` up to `depth` hops, as an indented tree.
fn transitive(
    graph: &CallGraph,
    symbol: &str,
    callers: bool,
    depth: usize,
    filter: Option<&str>,
) -> String {
    let mut nodes = if callers {
        graph.bfs_callers(symbol, depth)
    } else {
        graph.bfs_callees(symbol, depth)
    };
    if let Some(f) = filter {
        nodes.retain(|n| index_paths::graph_match_key(&n.file).contains(f));
    }
    let noun = if callers { "caller" } else { "callee" };

    if nodes.is_empty() {
        return format!(
            "No {noun}s found for '{symbol}' (depth≤{depth}, {} edges in graph)",
            graph.edges.len()
        );
    }

    let arrow = if callers { "←" } else { "→" };
    let mut out = format!("{} {noun}(s) of '{symbol}' (depth≤{depth}):\n", nodes.len());
    for node in &nodes {
        let indent = "  ".repeat(node.depth);
        out.push_str(&format!(
            "{indent}{} {arrow} {}  ({}:L{})\n",
            node.from_symbol, node.symbol, node.file, node.line
        ));
    }
    out
}

/// The shortest call chain from `from` to `to`.
fn call_path(graph: &CallGraph, from: &str, to: &str) -> String {
    let Some(hops) = graph.find_call_path(from, to) else {
        return format!("No call path found from '{from}' to '{to}' (searched up to depth 10)");
    };
    let mut out = format!("Call path ({} hop(s)):\n", hops.len() - 1);
    for (i, hop) in hops.iter().enumerate() {
        let loc = if hop.file.is_empty() {
            String::new()
        } else {
            format!("  ({}:L{})", hop.file, hop.line)
        };
        let arrow = if i == 0 { "" } else { "→ " };
        out.push_str(&format!("  {arrow}{}{loc}\n", hop.symbol));
    }
    out
}

/// Append the stable-handle usage hint (#607) to a non-empty result so the
/// agent can re-target any listed symbol via `sobre_search(action="symbol",
/// handle=…)`. Skips error/empty messages (which start with "No ") so failures
/// stay clean.
fn with_handle_hint(out: String) -> String {
    if out.starts_with("No ") || out.trim().is_empty() {
        out
    } else {
        format!("{out}{}\n", crate::core::handle::USAGE_HINT)
    }
}

fn graph_file_filter(file: &str, project_root: &str) -> String {
    let rel = index_paths::graph_relative_key(file, project_root);
    let rel_key = index_paths::graph_match_key(&rel);
    if rel_key.is_empty() {
        index_paths::graph_match_key(file)
    } else {
        rel_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_file_filter_normalizes_windows_styles() {
        let filter = graph_file_filter(r"C:/repo/src/main/kotlin/Example.kt", r"C:\repo");
        let expected = if cfg!(windows) {
            "src/main/kotlin/Example.kt"
        } else {
            "C:/repo/src/main/kotlin/Example.kt"
        };
        assert_eq!(filter, expected);
    }

    #[test]
    fn missing_inputs_return_errors() {
        let callers = callers_or_callees(None, true, None, 1, "/tmp");
        assert!(
            callers.contains("symbol is required for action=callers"),
            "{callers}"
        );
        let callees = callers_or_callees(None, false, None, 1, "/tmp");
        assert!(
            callees.contains("symbol is required for action=callees"),
            "{callees}"
        );
        let trace_from = trace(None, Some("b"), "/tmp");
        assert!(trace_from.contains("symbol is required"), "{trace_from}");
        let trace_to = trace(Some("a"), None, "/tmp");
        assert!(trace_to.contains("to is required"), "{trace_to}");
    }

    #[test]
    fn handle_hint_appended_to_results_only() {
        let ok = with_handle_hint("1 caller(s) of 'x':\n  a → b  (L1)\n".to_string());
        assert!(
            ok.contains("handle=\""),
            "success output gets the hint: {ok}"
        );
        let empty = with_handle_hint("No callers found for 'x' (0 edges in graph)".to_string());
        assert!(
            !empty.contains("handle=\""),
            "empty result stays clean: {empty}"
        );
    }
}
