use std::collections::HashMap;
use std::path::Path;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::deep_queries;
use super::graph_provider::GraphProvider;
use super::index_paths::normalize_project_root;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallGraph {
    pub project_root: String,
    pub edges: Vec<CallEdge>,
    pub file_hashes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallEdge {
    pub caller_file: String,
    pub caller_symbol: String,
    pub caller_line: usize,
    pub callee_name: String,
}

/// Minimal symbol span the call-graph builder needs to attribute a call site to
/// its enclosing symbol — backend-agnostic, decoupled from any graph store.
#[derive(Debug, Clone)]
pub struct SymbolSpan {
    pub file: String,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Everything the call-graph builder reads, sourced from the [`GraphProvider`]
/// facade (PropertyGraph): the file inventory and the symbol table (for
/// enclosing-symbol attribution). Source content itself is read fresh from
/// disk per file.
#[derive(Debug, Clone, Default)]
pub struct CallGraphInputs {
    pub project_root: String,
    pub file_paths: Vec<String>,
    pub symbols: Vec<SymbolSpan>,
}

impl CallGraphInputs {
    /// Open (or build) the property graph and materialize call-graph inputs.
    /// Returns empty inputs (rooted at `project_root`) when no graph is
    /// available.
    pub fn open(project_root: &str) -> Self {
        match crate::core::graph_provider::open_or_build(project_root) {
            Some(open) => Self::from_provider(project_root, &open),
            None => Self {
                project_root: normalize_project_root(project_root),
                ..Default::default()
            },
        }
    }

    /// Materialize the builder inputs from a [`GraphProvider`] facade.
    pub fn from_provider(project_root: &str, provider: &GraphProvider) -> Self {
        let symbols = provider
            .all_symbols()
            .into_iter()
            .map(|s| SymbolSpan {
                file: s.file,
                name: s.name,
                start_line: s.start_line,
                end_line: s.end_line,
            })
            .collect();
        Self {
            project_root: normalize_project_root(project_root),
            file_paths: provider.file_paths(),
            symbols,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BfsNode {
    pub symbol: String,
    pub file: String,
    pub line: usize,
    pub depth: usize,
    pub from_symbol: String,
}

#[derive(Debug, Clone)]
pub struct PathHop {
    pub symbol: String,
    pub file: String,
    pub line: usize,
}

#[derive(Clone, Copy)]
enum BfsDirection {
    Callers,
    Callees,
}

impl CallGraph {
    pub fn new(project_root: &str) -> Self {
        Self {
            project_root: normalize_project_root(project_root),
            edges: Vec::new(),
            file_hashes: HashMap::new(),
        }
    }

    /// Build the graph for `inputs`, re-analyzing only the files whose content
    /// hash differs from `previous` and reusing its edges for the others.
    fn build_incremental(inputs: &CallGraphInputs, previous: &CallGraph) -> Self {
        let project_root = &inputs.project_root;
        let symbols_by_file = group_symbols_by_file_owned(inputs);
        let file_keys: Vec<String> = inputs.file_paths.clone();

        let prev_edges_by_file = group_edges_by_file(&previous.edges);

        let results: Vec<(String, String, Vec<CallEdge>)> = file_keys
            .par_iter()
            .filter_map(|rel_path| {
                let abs_path = resolve_path(rel_path, project_root);
                let content = std::fs::read_to_string(&abs_path).ok()?;
                let hash = simple_hash(&content);
                let changed = previous.file_hashes.get(rel_path.as_str()) != Some(&hash);

                let edges = if changed {
                    let ext = Path::new(rel_path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");

                    let analysis = deep_queries::analyze(&content, ext);
                    let file_symbols = symbols_by_file.get(rel_path.as_str());

                    analysis
                        .calls
                        .iter()
                        .map(|call| {
                            let caller_sym = find_enclosing_symbol_owned(file_symbols, call.line);
                            CallEdge {
                                caller_file: rel_path.clone(),
                                caller_symbol: caller_sym,
                                caller_line: call.line,
                                callee_name: call.callee.clone(),
                            }
                        })
                        .collect()
                } else {
                    prev_edges_by_file
                        .get(rel_path.as_str())
                        .cloned()
                        .unwrap_or_default()
                };

                Some((rel_path.clone(), hash, edges))
            })
            .collect();

        let mut graph = Self::new(project_root);
        let edge_capacity: usize = results.iter().map(|(_, _, e)| e.len()).sum();
        graph.edges.reserve(edge_capacity);
        graph.file_hashes.reserve(results.len());

        for (path, hash, edges) in results {
            graph.file_hashes.insert(path, hash);
            graph.edges.extend(edges);
        }

        graph
    }

    pub fn callers_of(&self, symbol: &str) -> Vec<&CallEdge> {
        let sym_lower = symbol.to_lowercase();
        self.edges
            .iter()
            .filter(|e| e.callee_name.to_lowercase() == sym_lower)
            .collect()
    }

    pub fn callees_of(&self, symbol: &str) -> Vec<&CallEdge> {
        let sym_lower = symbol.to_lowercase();
        self.edges
            .iter()
            .filter(|e| e.caller_symbol.to_lowercase() == sym_lower)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Multi-hop BFS traversal
    // -----------------------------------------------------------------------

    /// BFS callers up to `max_depth` hops. Returns (symbol, file, line, depth) per node.
    pub fn bfs_callers(&self, symbol: &str, max_depth: usize) -> Vec<BfsNode> {
        self.bfs_traverse(symbol, max_depth, BfsDirection::Callers)
    }

    /// BFS callees up to `max_depth` hops. Returns (symbol, file, line, depth) per node.
    pub fn bfs_callees(&self, symbol: &str, max_depth: usize) -> Vec<BfsNode> {
        self.bfs_traverse(symbol, max_depth, BfsDirection::Callees)
    }

    fn bfs_traverse(&self, symbol: &str, max_depth: usize, dir: BfsDirection) -> Vec<BfsNode> {
        use std::collections::{HashSet, VecDeque};

        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let mut result: Vec<BfsNode> = Vec::new();

        let start = symbol.to_lowercase();
        visited.insert(start.clone());
        queue.push_back((start, 0));

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            let neighbors: Vec<&CallEdge> = match dir {
                BfsDirection::Callers => self
                    .edges
                    .iter()
                    .filter(|e| e.callee_name.to_lowercase() == current)
                    .collect(),
                BfsDirection::Callees => self
                    .edges
                    .iter()
                    .filter(|e| e.caller_symbol.to_lowercase() == current)
                    .collect(),
            };

            for edge in neighbors {
                let next_sym = match dir {
                    BfsDirection::Callers => &edge.caller_symbol,
                    BfsDirection::Callees => &edge.callee_name,
                };
                let next_lower = next_sym.to_lowercase();

                if !visited.insert(next_lower.clone()) {
                    continue;
                }

                result.push(BfsNode {
                    symbol: next_sym.clone(),
                    file: edge.caller_file.clone(),
                    line: edge.caller_line,
                    depth: depth + 1,
                    from_symbol: if depth == 0 {
                        symbol.to_string()
                    } else {
                        current.clone()
                    },
                });

                queue.push_back((next_lower, depth + 1));
            }
        }

        result
    }

    /// Find shortest call path from `from` to `to` using BFS.
    /// Returns None if no path exists (searched up to depth 10).
    pub fn find_call_path(&self, from: &str, to: &str) -> Option<Vec<PathHop>> {
        use std::collections::{HashMap as BfsMap, VecDeque};

        let from_lower = from.to_lowercase();
        let to_lower = to.to_lowercase();

        if from_lower == to_lower {
            return Some(vec![PathHop {
                symbol: from.to_string(),
                file: String::new(),
                line: 0,
            }]);
        }

        const MAX_TRACE_DEPTH: usize = 10;

        // (parent_symbol, file, line, depth)
        let mut visited: BfsMap<String, (String, String, usize, usize)> = BfsMap::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        visited.insert(from_lower.clone(), (String::new(), String::new(), 0, 0));
        queue.push_back(from_lower.clone());

        while let Some(current) = queue.pop_front() {
            let current_depth = visited.get(&current).map_or(0, |e| e.3);
            if current_depth >= MAX_TRACE_DEPTH {
                continue;
            }

            let callees: Vec<&CallEdge> = self
                .edges
                .iter()
                .filter(|e| e.caller_symbol.to_lowercase() == current)
                .collect();

            for edge in callees {
                let next = edge.callee_name.to_lowercase();
                if visited.contains_key(&next) {
                    continue;
                }

                visited.insert(
                    next.clone(),
                    (
                        current.clone(),
                        edge.caller_file.clone(),
                        edge.caller_line,
                        current_depth + 1,
                    ),
                );

                if next == to_lower {
                    return Some(Self::reconstruct_path(
                        &visited,
                        &from_lower,
                        &to_lower,
                        from,
                        to,
                    ));
                }

                queue.push_back(next);
            }
        }

        None
    }

    fn reconstruct_path(
        visited: &std::collections::HashMap<String, (String, String, usize, usize)>,
        from_lower: &str,
        to_lower: &str,
        from_orig: &str,
        to_orig: &str,
    ) -> Vec<PathHop> {
        let mut path = Vec::new();
        let mut current = to_lower.to_string();

        while current != from_lower {
            let (parent, file, line, _depth) = &visited[&current];
            let sym_name = if current == to_lower {
                to_orig.to_string()
            } else {
                current.clone()
            };
            path.push(PathHop {
                symbol: sym_name,
                file: file.clone(),
                line: *line,
            });
            current = parent.clone();
        }

        path.push(PathHop {
            symbol: from_orig.to_string(),
            file: String::new(),
            line: 0,
        });

        path.reverse();
        path
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = call_graph_dir(&self.project_root)
            .ok_or_else(|| "Cannot determine home directory".to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let compressed = zstd::encode_all(json.as_bytes(), 9).map_err(|e| format!("zstd: {e}"))?;
        let target = dir.join("call_graph.json.zst");
        let tmp = target.with_extension("zst.tmp");
        std::fs::write(&tmp, &compressed).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load(project_root: &str) -> Option<Self> {
        let dir = call_graph_dir(project_root)?;

        let compressed = std::fs::read(dir.join("call_graph.json.zst")).ok()?;
        let data = zstd::decode_all(compressed.as_slice()).ok()?;
        serde_json::from_str(&String::from_utf8(data).ok()?).ok()
    }

    /// Bring the persisted graph up to date with `inputs`; every file is
    /// analyzed when nothing is persisted yet.
    pub fn load_or_build(project_root: &str, inputs: &CallGraphInputs) -> Self {
        let previous = Self::load(project_root).unwrap_or_else(|| Self::new(project_root));
        Self::build_incremental(inputs, &previous)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn call_graph_dir(project_root: &str) -> Option<std::path::PathBuf> {
    GraphProvider::index_dir(project_root)
}

fn group_edges_by_file(edges: &[CallEdge]) -> HashMap<&str, Vec<CallEdge>> {
    let mut map: HashMap<&str, Vec<CallEdge>> = HashMap::new();
    for edge in edges {
        map.entry(edge.caller_file.as_str())
            .or_default()
            .push(edge.clone());
    }
    map
}

/// Owned version for safe `Send` across rayon threads.
fn group_symbols_by_file_owned(inputs: &CallGraphInputs) -> HashMap<String, Vec<SymbolSpan>> {
    let mut map: HashMap<String, Vec<SymbolSpan>> = HashMap::new();
    for sym in &inputs.symbols {
        map.entry(sym.file.clone()).or_default().push(sym.clone());
    }
    for syms in map.values_mut() {
        syms.sort_by_key(|s| s.start_line);
    }
    map
}

fn find_enclosing_symbol_owned(file_symbols: Option<&Vec<SymbolSpan>>, line: usize) -> String {
    let Some(syms) = file_symbols else {
        return "<module>".to_string();
    };
    let mut best: Option<&SymbolSpan> = None;
    for sym in syms {
        if line >= sym.start_line && line <= sym.end_line {
            match best {
                None => best = Some(sym),
                Some(prev) => {
                    if (sym.end_line - sym.start_line) < (prev.end_line - prev.start_line) {
                        best = Some(sym);
                    }
                }
            }
        }
    }
    best.map_or_else(|| "<module>".to_string(), |s| s.name.clone())
}

fn resolve_path(relative: &str, project_root: &str) -> String {
    let p = Path::new(relative);
    if p.is_absolute() && p.exists() {
        return relative.to_string();
    }
    let relative = relative.trim_start_matches(['/', '\\']);
    let joined = Path::new(project_root).join(relative);
    joined.to_string_lossy().to_string()
}

/// A file's content hash, saved with the call graph and compared on the next
/// load: stable across Rust toolchains.
fn simple_hash(content: &str) -> String {
    crate::core::hasher::hash_short(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_of_empty_graph() {
        let graph = CallGraph::new("/tmp");
        assert!(graph.callers_of("foo").is_empty());
    }

    #[test]
    fn callers_of_finds_edges() {
        let mut graph = CallGraph::new("/tmp");
        graph.edges.push(CallEdge {
            caller_file: "a.rs".to_string(),
            caller_symbol: "bar".to_string(),
            caller_line: 10,
            callee_name: "foo".to_string(),
        });
        graph.edges.push(CallEdge {
            caller_file: "b.rs".to_string(),
            caller_symbol: "baz".to_string(),
            caller_line: 20,
            callee_name: "foo".to_string(),
        });
        graph.edges.push(CallEdge {
            caller_file: "c.rs".to_string(),
            caller_symbol: "qux".to_string(),
            caller_line: 30,
            callee_name: "other".to_string(),
        });
        let callers = graph.callers_of("foo");
        assert_eq!(callers.len(), 2);
    }

    #[test]
    fn callees_of_finds_edges() {
        let mut graph = CallGraph::new("/tmp");
        graph.edges.push(CallEdge {
            caller_file: "a.rs".to_string(),
            caller_symbol: "main".to_string(),
            caller_line: 5,
            callee_name: "init".to_string(),
        });
        graph.edges.push(CallEdge {
            caller_file: "a.rs".to_string(),
            caller_symbol: "main".to_string(),
            caller_line: 6,
            callee_name: "run".to_string(),
        });
        graph.edges.push(CallEdge {
            caller_file: "a.rs".to_string(),
            caller_symbol: "other".to_string(),
            caller_line: 15,
            callee_name: "init".to_string(),
        });
        let callees = graph.callees_of("main");
        assert_eq!(callees.len(), 2);
    }

    #[test]
    fn find_enclosing_picks_narrowest() {
        let outer = SymbolSpan {
            file: "a.rs".to_string(),
            name: "Outer".to_string(),
            start_line: 1,
            end_line: 50,
        };
        let inner = SymbolSpan {
            file: "a.rs".to_string(),
            name: "inner_fn".to_string(),
            start_line: 10,
            end_line: 20,
        };
        let syms = vec![outer, inner];
        let result = find_enclosing_symbol_owned(Some(&syms), 15);
        assert_eq!(result, "inner_fn");
    }

    #[test]
    fn find_enclosing_returns_module_when_no_match() {
        let sym = SymbolSpan {
            file: "a.rs".to_string(),
            name: "foo".to_string(),
            start_line: 10,
            end_line: 20,
        };
        let syms = vec![sym];
        let result = find_enclosing_symbol_owned(Some(&syms), 5);
        assert_eq!(result, "<module>");
    }

    #[test]
    fn resolve_path_trims_rooted_relative_prefix() {
        let resolved = resolve_path(r"\src\main\kotlin\Example.kt", r"C:\repo");
        assert_eq!(
            resolved,
            Path::new(r"C:\repo")
                .join(r"src\main\kotlin\Example.kt")
                .to_string_lossy()
                .to_string()
        );
    }

    fn build_chain_graph() -> CallGraph {
        // A -> B -> C -> D
        let mut graph = CallGraph::new("/tmp");
        graph.edges.push(CallEdge {
            caller_file: "a.rs".into(),
            caller_symbol: "fn_a".into(),
            caller_line: 1,
            callee_name: "fn_b".into(),
        });
        graph.edges.push(CallEdge {
            caller_file: "b.rs".into(),
            caller_symbol: "fn_b".into(),
            caller_line: 10,
            callee_name: "fn_c".into(),
        });
        graph.edges.push(CallEdge {
            caller_file: "c.rs".into(),
            caller_symbol: "fn_c".into(),
            caller_line: 20,
            callee_name: "fn_d".into(),
        });
        graph
    }

    #[test]
    fn bfs_callees_depth_1_returns_direct() {
        let graph = build_chain_graph();
        let nodes = graph.bfs_callees("fn_a", 1);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].symbol, "fn_b");
        assert_eq!(nodes[0].depth, 1);
    }

    #[test]
    fn bfs_callees_depth_3_returns_chain() {
        let graph = build_chain_graph();
        let nodes = graph.bfs_callees("fn_a", 3);
        assert_eq!(nodes.len(), 3);
        let syms: Vec<&str> = nodes.iter().map(|n| n.symbol.as_str()).collect();
        assert!(syms.contains(&"fn_b"));
        assert!(syms.contains(&"fn_c"));
        assert!(syms.contains(&"fn_d"));
    }

    #[test]
    fn bfs_callers_depth_2_returns_transitive() {
        let graph = build_chain_graph();
        let nodes = graph.bfs_callers("fn_c", 2);
        assert_eq!(nodes.len(), 2);
        let syms: Vec<&str> = nodes.iter().map(|n| n.symbol.as_str()).collect();
        assert!(syms.contains(&"fn_b"));
        assert!(syms.contains(&"fn_a"));
    }

    #[test]
    fn find_call_path_direct() {
        let graph = build_chain_graph();
        let path = graph.find_call_path("fn_a", "fn_b");
        assert!(path.is_some());
        let hops = path.unwrap();
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].symbol, "fn_a");
        assert_eq!(hops[1].symbol, "fn_b");
    }

    #[test]
    fn find_call_path_multi_hop() {
        let graph = build_chain_graph();
        let path = graph.find_call_path("fn_a", "fn_d");
        assert!(path.is_some());
        let hops = path.unwrap();
        assert_eq!(hops.len(), 4);
        assert_eq!(hops[0].symbol, "fn_a");
        assert_eq!(hops[3].symbol, "fn_d");
    }

    #[test]
    fn find_call_path_no_connection() {
        let graph = build_chain_graph();
        let path = graph.find_call_path("fn_d", "fn_a");
        assert!(path.is_none());
    }

    #[test]
    fn find_call_path_same_symbol() {
        let graph = build_chain_graph();
        let path = graph.find_call_path("fn_a", "fn_a");
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 1);
    }

    #[test]
    fn bfs_handles_cycle_without_infinite_loop() {
        let mut graph = CallGraph::new("/tmp");
        graph.edges.push(CallEdge {
            caller_file: "a.rs".into(),
            caller_symbol: "a".into(),
            caller_line: 1,
            callee_name: "b".into(),
        });
        graph.edges.push(CallEdge {
            caller_file: "b.rs".into(),
            caller_symbol: "b".into(),
            caller_line: 2,
            callee_name: "a".into(),
        });
        let nodes = graph.bfs_callees("a", 5);
        // Should visit b once (depth 1), then a is already visited
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].symbol, "b");
    }
}
