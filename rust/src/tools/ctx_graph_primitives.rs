//! Graph primitives for `ctx_graph`: `neighbors` and `path` (shortest path),
//! traversal helpers over the file-level graph (`GraphProvider::edges`).
//!
//! Both accept `format="json"` for machine consumption; otherwise they emit
//! compact, token-light text with a `[ctx_graph <action>: N tok]` footer like the
//! other actions.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::core::graph_provider::{self, EdgeInfo};
use crate::core::index_paths;
use crate::core::protocol::shorten_path;
use crate::core::tokens::count_tokens;

/// One adjacency entry: a neighbour reached via an edge of `kind`/`weight`.
struct NeighborRef {
    node: String,
    kind: String,
    weight: f64,
}

/// A directed, file-level adjacency view built once from `GraphProvider::edges`.
/// Node ids are repo-relative file paths — identical to the dashboard graph.
struct Adj {
    nodes: Vec<String>,
    node_set: HashSet<String>,
    out: HashMap<String, Vec<NeighborRef>>,
    inc: HashMap<String, Vec<NeighborRef>>,
}

impl Adj {
    fn build(edges: &[EdgeInfo], file_paths: &[String]) -> Self {
        let mut out: HashMap<String, Vec<NeighborRef>> = HashMap::new();
        let mut inc: HashMap<String, Vec<NeighborRef>> = HashMap::new();
        let mut node_set: HashSet<String> = HashSet::new();
        for p in file_paths {
            node_set.insert(p.clone());
        }
        for e in edges {
            node_set.insert(e.from.clone());
            node_set.insert(e.to.clone());
            out.entry(e.from.clone()).or_default().push(NeighborRef {
                node: e.to.clone(),
                kind: e.kind.clone(),
                weight: e.weight,
            });
            inc.entry(e.to.clone()).or_default().push(NeighborRef {
                node: e.from.clone(),
                kind: e.kind.clone(),
                weight: e.weight,
            });
        }
        let mut nodes: Vec<String> = node_set.iter().cloned().collect();
        nodes.sort();
        Self {
            nodes,
            node_set,
            out,
            inc,
        }
    }

    /// Resolve a user-supplied path to a concrete graph node. Tries an exact
    /// repo-relative match first, then a unique path/basename suffix match.
    fn resolve(&self, input: &str, root: &str) -> Result<String, String> {
        let rel = index_paths::graph_relative_key(input, root);
        if self.node_set.contains(&rel) {
            return Ok(rel);
        }
        let needle = index_paths::graph_match_key(&rel);
        if self.node_set.contains(&needle) {
            return Ok(needle);
        }
        let base = needle.rsplit('/').next().unwrap_or(&needle).to_string();
        let suffix = format!("/{needle}");
        let base_suffix = format!("/{base}");
        let cands: Vec<&String> = self
            .nodes
            .iter()
            .filter(|n| {
                let nk = index_paths::graph_match_key(n);
                nk == needle || nk.ends_with(&suffix) || nk == base || nk.ends_with(&base_suffix)
            })
            .collect();
        match cands.len() {
            0 => Err(format!(
                "Node not found in graph: {input}\nRun ctx_graph action='build' to (re)index, or pass a path that exists in the project."
            )),
            1 => Ok(cands[0].clone()),
            _ => {
                // Show full repo-relative paths (not basenames) so the caller
                // can actually tell the candidates apart.
                let list = cands
                    .iter()
                    .take(10)
                    .map(|c| format!("  {c}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let more = if cands.len() > 10 {
                    format!("\n  … and {} more", cands.len() - 10)
                } else {
                    String::new()
                };
                Err(format!(
                    "'{input}' is ambiguous ({} matches) — pass a more specific path:\n{list}{more}",
                    cands.len()
                ))
            }
        }
    }

    fn outgoing(&self, node: &str) -> Vec<&NeighborRef> {
        let mut v: Vec<&NeighborRef> = self
            .out
            .get(node)
            .map(|x| x.iter().collect())
            .unwrap_or_default();
        v.sort_by(|a, b| a.node.cmp(&b.node).then_with(|| a.kind.cmp(&b.kind)));
        v
    }

    fn incoming(&self, node: &str) -> Vec<&NeighborRef> {
        let mut v: Vec<&NeighborRef> = self
            .inc
            .get(node)
            .map(|x| x.iter().collect())
            .unwrap_or_default();
        v.sort_by(|a, b| a.node.cmp(&b.node).then_with(|| a.kind.cmp(&b.kind)));
        v
    }

    /// Unique undirected neighbours (out ∪ inc), deterministically sorted.
    fn undirected_neighbors(&self, node: &str) -> Vec<String> {
        let mut set: HashSet<&str> = HashSet::new();
        if let Some(v) = self.out.get(node) {
            for nb in v {
                set.insert(nb.node.as_str());
            }
        }
        if let Some(v) = self.inc.get(node) {
            for nb in v {
                set.insert(nb.node.as_str());
            }
        }
        let mut out: Vec<String> = set.into_iter().map(str::to_string).collect();
        out.sort();
        out
    }

    /// The strongest edge directly connecting `a` and `b`, with its direction.
    /// Direction: `Forward` = a→b, `Backward` = b→a.
    fn edge_between(&self, a: &str, b: &str) -> Option<(Direction, String, f64)> {
        let mut best: Option<(Direction, String, f64)> = None;
        let mut consider = |dir: Direction, kind: &str, weight: f64| {
            let conf = edge_confidence(kind, weight);
            if best.as_ref().is_none_or(|(_, _, c)| conf > *c) {
                best = Some((dir, kind.to_string(), conf));
            }
        };
        if let Some(v) = self.out.get(a) {
            for nb in v.iter().filter(|nb| nb.node == b) {
                consider(Direction::Forward, &nb.kind, nb.weight);
            }
        }
        if let Some(v) = self.inc.get(a) {
            for nb in v.iter().filter(|nb| nb.node == b) {
                consider(Direction::Backward, &nb.kind, nb.weight);
            }
        }
        best
    }

    /// Shortest undirected path `from → to` (BFS, deterministic). Includes both
    /// endpoints. `None` when the two nodes are in different components.
    fn bfs_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        if from == to {
            return Some(vec![from.to_string()]);
        }
        let mut prev: HashMap<String, String> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        visited.insert(from.to_string());
        queue.push_back(from.to_string());
        while let Some(cur) = queue.pop_front() {
            for nb in self.undirected_neighbors(&cur) {
                if visited.contains(&nb) {
                    continue;
                }
                visited.insert(nb.clone());
                prev.insert(nb.clone(), cur.clone());
                if nb == to {
                    return Some(reconstruct(&prev, from, to));
                }
                queue.push_back(nb);
            }
        }
        None
    }

    /// BFS distance rings from `start` (undirected), capped at `max_depth`.
    /// Returns distance → sorted node list (excludes the start node).
    fn bfs_rings(&self, start: &str, max_depth: usize) -> Vec<(usize, Vec<String>)> {
        let mut dist: HashMap<String, usize> = HashMap::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        dist.insert(start.to_string(), 0);
        queue.push_back(start.to_string());
        while let Some(cur) = queue.pop_front() {
            let d = dist[&cur];
            if d >= max_depth {
                continue;
            }
            for nb in self.undirected_neighbors(&cur) {
                if !dist.contains_key(&nb) {
                    dist.insert(nb.clone(), d + 1);
                    queue.push_back(nb);
                }
            }
        }
        let mut rings: HashMap<usize, Vec<String>> = HashMap::new();
        for (node, d) in dist {
            if d == 0 {
                continue;
            }
            rings.entry(d).or_default().push(node);
        }
        let mut out: Vec<(usize, Vec<String>)> = rings
            .into_iter()
            .map(|(d, mut nodes)| {
                nodes.sort();
                (d, nodes)
            })
            .collect();
        out.sort_by_key(|(d, _)| *d);
        out
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Direction {
    Forward,
    Backward,
}

impl Direction {
    fn arrow(self) -> &'static str {
        match self {
            Direction::Forward => "->",
            Direction::Backward => "<-",
        }
    }
}

fn reconstruct(prev: &HashMap<String, String>, from: &str, to: &str) -> Vec<String> {
    let mut chain = vec![to.to_string()];
    let mut cur = to.to_string();
    while cur != from {
        match prev.get(&cur) {
            Some(p) => {
                chain.push(p.clone());
                cur = p.clone();
            }
            None => break,
        }
    }
    chain.reverse();
    chain
}

fn open_graph(root: &str) -> Result<graph_provider::GraphProvider, String> {
    graph_provider::open_or_build(root)
        .ok_or_else(|| "No graph index found. Run ctx_graph with action='build' first.".to_string())
}

fn is_json(format: Option<&str>) -> bool {
    matches!(format, Some(f) if f.eq_ignore_ascii_case("json"))
}

/// `ctx_graph action=neighbors` — immediate (and optionally multi-hop) graph
/// neighbours of a file, split by direction and annotated with edge kind.
pub fn neighbors(
    path: Option<&str>,
    root: &str,
    depth: Option<usize>,
    format: Option<&str>,
) -> String {
    let Some(input) = path else {
        return "path is required for 'neighbors' action".to_string();
    };
    let open = match open_graph(root) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let gp = &open;
    let adj = Adj::build(&gp.edges(), &gp.file_paths());
    let node = match adj.resolve(input, root) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let depth = depth.unwrap_or(1).clamp(1, 6);
    let outgoing = adj.outgoing(&node);
    let incoming = adj.incoming(&node);
    let rings = if depth > 1 {
        adj.bfs_rings(&node, depth)
    } else {
        Vec::new()
    };

    if is_json(format) {
        let out_json: Vec<_> = outgoing
            .iter()
            .map(|n| {
                serde_json::json!({
                    "node": n.node,
                    "kind": n.kind,
                    "confidence": round3(edge_confidence(&n.kind, n.weight)),
                })
            })
            .collect();
        let in_json: Vec<_> = incoming
            .iter()
            .map(|n| {
                serde_json::json!({
                    "node": n.node,
                    "kind": n.kind,
                    "confidence": round3(edge_confidence(&n.kind, n.weight)),
                })
            })
            .collect();
        let rings_json: Vec<_> = rings
            .iter()
            .map(|(d, nodes)| serde_json::json!({ "distance": d, "count": nodes.len(), "nodes": nodes }))
            .collect();
        let val = serde_json::json!({
            "node": node,
            "outgoing": out_json,
            "incoming": in_json,
            "rings": rings_json,
        });
        return serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string());
    }

    let mut out = format!("Neighbors of {}\n", shorten_path(&node));
    out.push_str(&format!(
        "\nOutgoing ({}) — this file depends on / references:\n",
        outgoing.len()
    ));
    if outgoing.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for n in &outgoing {
            out.push_str(&format!(
                "  -> {:<48} {:<10} conf {:.2}\n",
                shorten_path(&n.node),
                n.kind,
                edge_confidence(&n.kind, n.weight)
            ));
        }
    }
    out.push_str(&format!(
        "\nIncoming ({}) — files that depend on / reference this:\n",
        incoming.len()
    ));
    if incoming.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for n in &incoming {
            out.push_str(&format!(
                "  <- {:<48} {:<10} conf {:.2}\n",
                shorten_path(&n.node),
                n.kind,
                edge_confidence(&n.kind, n.weight)
            ));
        }
    }
    if depth > 1 {
        let total: usize = rings.iter().map(|(_, n)| n.len()).sum();
        out.push_str(&format!("\nReachable within {depth} hops: {total} nodes\n"));
        for (d, nodes) in &rings {
            out.push_str(&format!("  {} hop(s): {} nodes\n", d, nodes.len()));
        }
    }
    let tokens = count_tokens(&out);
    format!("{out}[ctx_graph neighbors: {tokens} tok]")
}

/// `ctx_graph action=path` — shortest connection between two files, with the
/// edge kind/direction of each hop.
pub fn shortest_path(
    from: Option<&str>,
    to: Option<&str>,
    root: &str,
    format: Option<&str>,
) -> String {
    let (Some(a), Some(b)) = (from, to) else {
        return "Both 'path' (from) and 'to' are required for 'path' action".to_string();
    };
    let open = match open_graph(root) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let gp = &open;
    let adj = Adj::build(&gp.edges(), &gp.file_paths());
    let na = match adj.resolve(a, root) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let nb = match adj.resolve(b, root) {
        Ok(n) => n,
        Err(e) => return e,
    };

    let Some(chain) = adj.bfs_path(&na, &nb) else {
        if is_json(format) {
            return serde_json::json!({
                "from": na, "to": nb, "connected": false, "path": [],
            })
            .to_string();
        }
        return format!(
            "No path between {} and {} — they live in different components of the dependency graph.",
            shorten_path(&na),
            shorten_path(&nb)
        );
    };

    let hops = chain.len().saturating_sub(1);
    if is_json(format) {
        let steps: Vec<_> = chain
            .windows(2)
            .map(|w| {
                let (dir, kind, conf) = adj.edge_between(&w[0], &w[1]).unwrap_or((
                    Direction::Forward,
                    "related".to_string(),
                    0.5,
                ));
                serde_json::json!({
                    "from": w[0],
                    "to": w[1],
                    "direction": if dir == Direction::Forward { "forward" } else { "backward" },
                    "kind": kind,
                    "confidence": round3(conf),
                })
            })
            .collect();
        let val = serde_json::json!({
            "from": na, "to": nb, "connected": true, "hops": hops,
            "path": chain, "steps": steps,
        });
        return serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string());
    }

    let mut out = format!(
        "Shortest path {} -> {} ({} hops):\n\n",
        shorten_path(&na),
        shorten_path(&nb),
        hops
    );
    out.push_str(&format!("  {}\n", shorten_path(&chain[0])));
    for w in chain.windows(2) {
        let (dir, kind, conf) = adj.edge_between(&w[0], &w[1]).unwrap_or((
            Direction::Forward,
            "related".to_string(),
            0.5,
        ));
        out.push_str(&format!(
            "    {} {} (conf {:.2})\n  {}\n",
            dir.arrow(),
            kind,
            conf,
            shorten_path(&w[1])
        ));
    }
    let tokens = count_tokens(&out);
    format!("{out}[ctx_graph path: {tokens} tok]")
}

/// Confidence that an edge represents a *real* relationship, in `0.0..=1.0`.
///
/// Explicit code references (`import`/`reexport`) are certain; structural
/// `module` grouping is fairly reliable; the co-location/co-change heuristics
/// are weaker and scale with their observed `weight` (how often two files
/// changed together).
fn edge_confidence(kind: &str, weight: f64) -> f64 {
    match kind {
        "import" => 1.0,
        "reexport" => 0.95,
        "module" => 0.6,
        // More co-changes ⇒ more confidence, with diminishing returns.
        "cochange" => (0.30 + weight.max(0.0).ln_1p() * 0.18).clamp(0.30, 0.85),
        // Learned co-access (files opened together in real sessions, #289).
        "co_access" => (0.35 + weight.max(0.0).ln_1p() * 0.16).clamp(0.35, 0.80),
        "sibling" => 0.25,
        _ => 0.5,
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, kind: &str) -> EdgeInfo {
        EdgeInfo {
            from: from.into(),
            to: to.into(),
            kind: kind.into(),
            weight: 1.0,
        }
    }

    /// a -> b -> c, plus an isolated d. Used by most traversal tests.
    fn sample() -> Adj {
        let edges = vec![
            edge("src/a.rs", "src/b.rs", "import"),
            edge("src/b.rs", "src/c.rs", "import"),
        ];
        let files = vec![
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
            "src/c.rs".to_string(),
            "src/d.rs".to_string(),
        ];
        Adj::build(&edges, &files)
    }

    #[test]
    fn resolve_exact_and_basename() {
        let adj = sample();
        // Exact repo-relative match.
        assert_eq!(adj.resolve("src/a.rs", "/proj").unwrap(), "src/a.rs");
        // Unique basename match.
        assert_eq!(adj.resolve("c.rs", "/proj").unwrap(), "src/c.rs");
    }

    #[test]
    fn resolve_unknown_errors() {
        let adj = sample();
        assert!(adj.resolve("nope.rs", "/proj").is_err());
    }

    #[test]
    fn resolve_ambiguous_errors() {
        let edges = vec![edge("a/mod.rs", "b/mod.rs", "import")];
        let files = vec!["a/mod.rs".to_string(), "b/mod.rs".to_string()];
        let adj = Adj::build(&edges, &files);
        let err = adj.resolve("mod.rs", "/proj").unwrap_err();
        assert!(err.contains("ambiguous"), "got: {err}");
    }

    #[test]
    fn bfs_path_finds_shortest_chain() {
        let adj = sample();
        let path = adj.bfs_path("src/a.rs", "src/c.rs").unwrap();
        assert_eq!(path, vec!["src/a.rs", "src/b.rs", "src/c.rs"]);
    }

    #[test]
    fn bfs_path_is_undirected() {
        // Reverse direction still connects (edges are followed both ways).
        let adj = sample();
        let path = adj.bfs_path("src/c.rs", "src/a.rs").unwrap();
        assert_eq!(path, vec!["src/c.rs", "src/b.rs", "src/a.rs"]);
    }

    #[test]
    fn bfs_path_none_when_disconnected() {
        let adj = sample();
        assert!(adj.bfs_path("src/a.rs", "src/d.rs").is_none());
    }

    #[test]
    fn bfs_path_same_node_is_singleton() {
        let adj = sample();
        assert_eq!(
            adj.bfs_path("src/b.rs", "src/b.rs").unwrap(),
            vec!["src/b.rs"]
        );
    }

    #[test]
    fn rings_group_by_distance() {
        let adj = sample();
        let rings = adj.bfs_rings("src/a.rs", 3);
        assert_eq!(rings[0], (1, vec!["src/b.rs".to_string()]));
        assert_eq!(rings[1], (2, vec!["src/c.rs".to_string()]));
    }

    #[test]
    fn edge_between_reports_direction() {
        let adj = sample();
        let (dir, kind, conf) = adj.edge_between("src/a.rs", "src/b.rs").unwrap();
        assert_eq!(dir, Direction::Forward);
        assert_eq!(kind, "import");
        assert!((conf - 1.0).abs() < 1e-9);
        // Reverse view is Backward.
        let (dir2, _, _) = adj.edge_between("src/b.rs", "src/a.rs").unwrap();
        assert_eq!(dir2, Direction::Backward);
    }

    #[test]
    fn edge_between_prefers_higher_confidence() {
        // Two parallel edges: a weak sibling and a strong import. import wins.
        let edges = vec![
            edge("x.rs", "y.rs", "sibling"),
            edge("x.rs", "y.rs", "import"),
        ];
        let adj = Adj::build(&edges, &[]);
        let (_, kind, conf) = adj.edge_between("x.rs", "y.rs").unwrap();
        assert_eq!(kind, "import");
        assert!((conf - 1.0).abs() < 1e-9);
    }

    #[test]
    fn neighbors_split_in_and_out() {
        let adj = sample();
        let out = adj.outgoing("src/b.rs");
        let inc = adj.incoming("src/b.rs");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node, "src/c.rs");
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].node, "src/a.rs");
    }
}
