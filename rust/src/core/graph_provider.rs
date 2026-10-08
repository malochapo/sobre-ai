//! The one way tools read the code graph: the SQLite property graph that
//! `graph_index::scan` builds and mirrors (see `property_graph::sync`).

use super::graph_index::{self, ProjectIndex};
use super::property_graph::CodeGraph;

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub file: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub is_exported: bool,
}

/// Normalize a property-graph edge-kind string back to the graph_index spelling.
/// The mirror maps graph_index `import` → `EdgeKind::Imports`, which serializes
/// as the plural `imports`; consumers filter on the singular `import`, so
/// reverse exactly that one rename.
fn index_edge_kind(pg_kind: &str) -> String {
    match pg_kind {
        "imports" => "import".to_string(),
        other => other.to_string(),
    }
}

/// Absolute distance from a symbol's `start` line to a handle's hint `line`.
/// A missing hint contributes zero distance, so handles without an `@Lline`
/// suffix neither help nor hurt the tiebreak (all candidates rank equal here).
fn line_distance(start: usize, target: Option<usize>) -> usize {
    match target {
        Some(t) => start.abs_diff(t),
        None => 0,
    }
}

/// Convert a property-graph symbol [`Node`] into a [`SymbolInfo`], recovering
/// the precise source `kind` and export flag from the node metadata (the `Node`
/// itself only carries a coarse `NodeKind`).
///
/// [`Node`]: super::property_graph::Node
fn symbol_info_from_node(n: super::property_graph::Node) -> SymbolInfo {
    let (meta_kind, meta_exported) =
        super::property_graph::parse_symbol_metadata(n.metadata.as_deref());
    SymbolInfo {
        kind: meta_kind.unwrap_or_else(|| n.kind.as_str().to_string()),
        is_exported: meta_exported.unwrap_or(true),
        name: n.name,
        file: n.file_path,
        start_line: n.line_start.unwrap_or(0),
        end_line: n.line_end.unwrap_or(0),
    }
}

#[derive(Debug, Clone)]
pub struct EdgeInfo {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub weight: f64,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub hash: String,
    pub language: String,
    pub line_count: usize,
    pub token_count: usize,
    pub exports: Vec<String>,
    pub summary: String,
}

/// An opened, built code graph.
pub struct GraphProvider {
    graph: CodeGraph,
    project_root: String,
}

impl GraphProvider {
    pub(crate) fn new(graph: CodeGraph, project_root: &str) -> Self {
        Self {
            graph,
            project_root: project_root.to_string(),
        }
    }

    /// A provider over an in-memory graph holding `index`.
    #[cfg(test)]
    pub(crate) fn from_index(index: &ProjectIndex) -> Self {
        let graph = CodeGraph::open_in_memory().expect("in-memory graph");
        super::property_graph::populate_from_project_index(&graph, index).expect("populate");
        Self::new(graph, &index.project_root)
    }

    pub fn edge_count(&self) -> Option<usize> {
        self.graph.edge_count().ok()
    }

    /// Files that reference `file_path`.
    pub fn dependents(&self, file_path: &str) -> Vec<String> {
        self.graph.dependents(file_path).unwrap_or_default()
    }

    /// Files within `depth` hops of `file_path` over references and
    /// heuristics (co-change, same directory), in either direction.
    pub fn related(&self, file_path: &str, depth: usize) -> Vec<String> {
        const PER_HOP: usize = 50;
        let mut seen = std::collections::HashSet::from([file_path.to_string()]);
        let mut out = Vec::new();
        let mut frontier = vec![file_path.to_string()];
        for _ in 0..depth {
            let mut next = Vec::new();
            for f in &frontier {
                for (p, _) in self.related_files_scored(f, PER_HOP) {
                    if seen.insert(p.clone()) {
                        out.push(p.clone());
                        next.push(p);
                    }
                }
            }
            frontier = next;
        }
        out
    }

    /// Direct neighbors over references and heuristics, strongest first.
    pub fn related_files_scored(&self, file_path: &str, limit: usize) -> Vec<(String, f64)> {
        self.graph
            .related_files(file_path, limit)
            .unwrap_or_default()
    }

    pub fn file_paths(&self) -> Vec<String> {
        self.graph.file_catalog_paths().unwrap_or_default()
    }

    pub fn file_count(&self) -> usize {
        self.graph.file_catalog_count().unwrap_or(0)
    }

    pub fn symbol_count(&self) -> usize {
        self.graph.symbol_count().unwrap_or(0)
    }

    pub fn find_symbols(
        &self,
        name: &str,
        file_filter: Option<&str>,
        kind_filter: Option<&str>,
    ) -> Vec<SymbolInfo> {
        // Kind is filtered after mapping, not in SQL: the `nodes.kind` column is
        // the coarse `NodeKind` (every code symbol is `"symbol"`), while the
        // precise source kind (`struct`, `fn`, …) lives in metadata (#889).
        self.graph
            .find_symbols(name, file_filter, None)
            .unwrap_or_default()
            .into_iter()
            .map(symbol_info_from_node)
            .filter(|s| kind_filter.is_none_or(|k| s.kind == k))
            .collect()
    }

    /// Every symbol with its file + line span (unfiltered). Used by the
    /// call-graph builder to attribute call sites to their enclosing symbol.
    pub fn all_symbols(&self) -> Vec<SymbolInfo> {
        self.graph
            .all_symbols()
            .unwrap_or_default()
            .into_iter()
            .map(symbol_info_from_node)
            .collect()
    }

    pub fn get_symbol(&self, key: &str) -> Option<SymbolInfo> {
        // Keys are `rel_path::sym_name`. A file path never contains `::`, but a
        // symbol name does for trait impls (`std::fmt::Display for T`), so split
        // on the first `::`.
        let (file_path, sym_name) = key.split_once("::")?;
        self.graph
            .get_node_by_symbol(sym_name, file_path)
            .ok()
            .flatten()
            .map(symbol_info_from_node)
    }

    /// Resolve a stable [`SymbolHandle`](crate::core::handle::SymbolHandle) to
    /// its current [`SymbolInfo`], robust to line drift. Resolution order:
    ///
    /// 1. Exact `(path, name)`, ignoring the `@Lline` hint (the symbol may have
    ///    moved since the handle was emitted).
    /// 2. Otherwise, same-file candidates whose name matches exactly or by its
    ///    unqualified tail (`Config::load` ↔ `load`), ranked by exact-name
    ///    first, then nearest line to the handle hint, then lowest line, then
    ///    name — a total, deterministic order (#498).
    ///
    /// Returns `None` only when no symbol in that file plausibly matches.
    pub fn find_symbol_by_handle(
        &self,
        handle: &crate::core::handle::SymbolHandle,
    ) -> Option<SymbolInfo> {
        let key = format!("{}::{}", handle.path, handle.name);
        if let Some(sym) = self.get_symbol(&key) {
            return Some(sym);
        }

        let tail = handle
            .name
            .rsplit("::")
            .next()
            .unwrap_or(handle.name.as_str());
        let mut candidates: Vec<SymbolInfo> = self
            .all_symbols()
            .into_iter()
            .filter(|s| s.file == handle.path)
            .filter(|s| s.name == handle.name || s.name.rsplit("::").next() == Some(tail))
            .collect();
        candidates.sort_by(|a, b| {
            let exact_a = u8::from(a.name != handle.name);
            let exact_b = u8::from(b.name != handle.name);
            exact_a
                .cmp(&exact_b)
                .then_with(|| {
                    line_distance(a.start_line, handle.line)
                        .cmp(&line_distance(b.start_line, handle.line))
                })
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.name.cmp(&b.name))
        });
        candidates.into_iter().next()
    }

    /// Every file edge, kinds in the graph_index spelling (`import`, …).
    pub fn edges(&self) -> Vec<EdgeInfo> {
        self.graph
            .all_edges_flat()
            .unwrap_or_default()
            .into_iter()
            .map(|(from, to, kind, weight)| EdgeInfo {
                from,
                to,
                kind: index_edge_kind(&kind),
                weight,
            })
            .collect()
    }

    pub fn edges_by_kind(&self, kind: &str) -> Vec<EdgeInfo> {
        self.edges()
            .into_iter()
            .filter(|e| e.kind == kind)
            .collect()
    }

    /// Every catalogued file as [`FileInfo`].
    pub fn file_entries(&self) -> Vec<FileInfo> {
        self.file_paths()
            .into_iter()
            .filter_map(|p| self.get_file_entry(&p))
            .collect()
    }

    pub fn get_file_entry(&self, path: &str) -> Option<FileInfo> {
        self.graph
            .get_file_catalog(path)
            .ok()
            .flatten()
            .map(|e| FileInfo {
                path: e.path,
                hash: e.hash,
                language: e.language,
                line_count: e.line_count,
                token_count: e.token_count,
                exports: e.exports,
                summary: e.summary,
            })
    }

    /// When the graph was built, local time, or empty when unknown.
    pub fn last_scan(&self) -> String {
        super::property_graph::load_meta(&self.project_root)
            .and_then(|m| chrono::DateTime::parse_from_rfc3339(&m.built_at).ok())
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_default()
    }

    /// Reconstruct a [`ProjectIndex`] from the graph — the inverse of the mirror
    /// (`populate_from_project_index`). `graph_index` uses it to reuse unchanged
    /// files on a rescan.
    pub fn materialize_project_index(&self) -> ProjectIndex {
        let mut idx = ProjectIndex::new(&self.project_root);
        // Stamp `last_scan` from the build time so the TTL staleness check
        // reflects the real build age, not this materialization moment.
        let built = self.last_scan();
        if !built.is_empty() {
            idx.last_scan = built;
        }
        for f in self.file_entries() {
            idx.files.insert(
                f.path.clone(),
                graph_index::FileEntry {
                    path: f.path,
                    hash: f.hash,
                    language: f.language,
                    line_count: f.line_count,
                    token_count: f.token_count,
                    exports: f.exports,
                    summary: f.summary,
                },
            );
        }
        for s in self.all_symbols() {
            let key = format!("{}::{}", s.file, s.name);
            idx.symbols.insert(
                key,
                graph_index::SymbolEntry {
                    file: s.file,
                    name: s.name,
                    kind: s.kind,
                    start_line: s.start_line,
                    end_line: s.end_line,
                    is_exported: s.is_exported,
                },
            );
        }
        for e in self.edges() {
            idx.edges.push(graph_index::IndexEdge {
                from: e.from,
                to: e.to,
                kind: e.kind,
                weight: e.weight as f32,
            });
        }
        idx.rebuild_interner();
        idx
    }

    pub fn index_dir(project_root: &str) -> Option<std::path::PathBuf> {
        graph_index::ProjectIndex::index_dir(project_root)
    }
}

/// Open the built graph, or `None` when there is none yet. Never builds:
/// callers whose answer only improves with a graph (the `sobre_read` context
/// gate, search ranking, compaction, `sobre_graph status`) use this; tools that
/// need one to answer at all use [`open_or_build`].
pub fn open_best_effort(project_root: &str) -> Option<GraphProvider> {
    let graph = CodeGraph::open(project_root).ok()?;
    if graph.file_catalog_count().unwrap_or(0) == 0 {
        return None;
    }
    Some(GraphProvider::new(graph, project_root))
}

/// Maximum time `open_or_build` waits for a build (#1018). The build keeps
/// running past it and writes the graph when done.
const BUILD_TIMEOUT_SECS: u64 = 30;

/// Only one synchronous build at a time; a caller that finds one running
/// returns `None` instead of queueing.
static BUILD_GATE: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// Open the graph, building it first when there is none. `None` when the build
/// is already running elsewhere, timed out, or found no files.
pub fn open_or_build(project_root: &str) -> Option<GraphProvider> {
    if let Some(p) = open_best_effort(project_root) {
        return Some(p);
    }

    let Ok(_gate) = BUILD_GATE.try_lock() else {
        tracing::info!("open_or_build: another build in progress for {project_root}");
        return None;
    };

    let root_owned = project_root.to_string();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let idx = graph_index::load_or_build(&root_owned);
        let _ = tx.send(idx.files.is_empty());
    });

    match rx.recv_timeout(std::time::Duration::from_secs(BUILD_TIMEOUT_SECS)) {
        Ok(false) => open_best_effort(project_root),
        Ok(true) => None,
        Err(_) => {
            tracing::warn!(
                "open_or_build: graph build still running after {BUILD_TIMEOUT_SECS}s for {project_root}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_effort_none_when_no_graphs() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).expect("mkdir data");
        crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

        let project_root = tmp.path().join("proj");
        std::fs::create_dir_all(&project_root).expect("mkdir proj");
        let root = project_root.to_string_lossy().to_string();

        let open = open_best_effort(&root);
        assert!(open.is_none());

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    fn handle_provider() -> GraphProvider {
        let mut idx = ProjectIndex::new("/test");
        for (key, file, name, kind, start, end) in [
            (
                "src/lib.rs::Config",
                "src/lib.rs",
                "Config",
                "struct",
                5usize,
                20usize,
            ),
            (
                "src/lib.rs::Config::load",
                "src/lib.rs",
                "Config::load",
                "method",
                22,
                35,
            ),
            ("src/main.rs::main", "src/main.rs", "main", "fn", 1, 10),
        ] {
            idx.symbols.insert(
                key.to_string(),
                graph_index::SymbolEntry {
                    file: file.to_string(),
                    name: name.to_string(),
                    kind: kind.to_string(),
                    start_line: start,
                    end_line: end,
                    is_exported: true,
                },
            );
        }
        GraphProvider::from_index(&idx)
    }

    #[test]
    fn handle_resolves_exact_file_and_name() {
        let gp = handle_provider();
        let h = crate::core::handle::SymbolHandle::new("src/lib.rs", "Config::load", 22);
        let sym = gp.find_symbol_by_handle(&h).expect("resolves");
        assert_eq!(sym.name, "Config::load");
        assert_eq!(sym.start_line, 22);
    }

    #[test]
    fn handle_resolves_after_line_drift() {
        // Same (path, name) but a stale, drifted line — must still resolve to
        // the current symbol at its real line (robust beyond line-only refs).
        let gp = handle_provider();
        let h = crate::core::handle::SymbolHandle::new("src/lib.rs", "Config::load", 999);
        let sym = gp
            .find_symbol_by_handle(&h)
            .expect("resolves despite drift");
        assert_eq!(sym.start_line, 22);
    }

    #[test]
    fn handle_resolves_by_unqualified_tail() {
        // Handle carries only the unqualified tail `load`; the exact key misses,
        // so the same-file tail match + line tiebreak recovers `Config::load`.
        let gp = handle_provider();
        let h = crate::core::handle::SymbolHandle::new("src/lib.rs", "load", 22);
        let sym = gp.find_symbol_by_handle(&h).expect("resolves by tail");
        assert_eq!(sym.name, "Config::load");
    }

    #[test]
    fn handle_unknown_file_returns_none() {
        let gp = handle_provider();
        let h = crate::core::handle::SymbolHandle::new("src/nope.rs", "Config::load", 22);
        assert!(gp.find_symbol_by_handle(&h).is_none());
    }

    #[test]
    fn pg_kind_filter_matches_precise_metadata_kind() {
        // #889: a PG symbol's coarse `nodes.kind` is always `"symbol"`; the
        // source kind (`struct`) lives in metadata. Filtering by kind="struct"
        // must still find it (previously matched the coarse column → empty).
        use super::super::property_graph::{Node, NodeKind};

        let pg = CodeGraph::open_in_memory().unwrap();
        pg.upsert_node(
            &Node::symbol("State", "simplex/simplex.go", NodeKind::Symbol)
                .with_lines(129, 140)
                .with_metadata(r#"{"kind":"struct","exported":true}"#),
        )
        .unwrap();
        let gp = GraphProvider::new(pg, "/test");

        let hits = gp.find_symbols("State", None, Some("struct"));
        assert_eq!(
            hits.len(),
            1,
            "kind=struct must resolve the exported struct"
        );
        assert_eq!(hits[0].kind, "struct");
        assert!(hits[0].is_exported);

        // A non-matching kind still filters it out.
        assert!(gp.find_symbols("State", None, Some("fn")).is_empty());
    }

    #[test]
    fn materialize_project_index_round_trips_losslessly() {
        use super::super::graph_index::{FileEntry, IndexEdge, SymbolEntry};
        use super::super::property_graph::populate_from_project_index;

        let mut a = ProjectIndex::new("/test");
        a.files.insert(
            "src/a.rs".to_string(),
            FileEntry {
                path: "src/a.rs".to_string(),
                hash: "hash-a".to_string(),
                language: "rs".to_string(),
                line_count: 42,
                token_count: 137,
                exports: vec!["Foo".to_string()],
                summary: "module a".to_string(),
            },
        );
        a.files.insert(
            "src/b.rs".to_string(),
            FileEntry {
                path: "src/b.rs".to_string(),
                hash: "hash-b".to_string(),
                language: "rs".to_string(),
                line_count: 7,
                token_count: 19,
                exports: vec![],
                summary: String::new(),
            },
        );
        // Two symbols with DIFFERENT kinds and export flags — the fields most at
        // risk of being flattened by the coarse property-graph `NodeKind`.
        a.symbols.insert(
            "src/a.rs::Foo".to_string(),
            SymbolEntry {
                file: "src/a.rs".to_string(),
                name: "Foo".to_string(),
                kind: "struct".to_string(),
                start_line: 1,
                end_line: 9,
                is_exported: true,
            },
        );
        a.symbols.insert(
            "src/b.rs::helper".to_string(),
            SymbolEntry {
                file: "src/b.rs".to_string(),
                name: "helper".to_string(),
                kind: "function".to_string(),
                start_line: 3,
                end_line: 6,
                is_exported: false,
            },
        );
        a.edges.push(IndexEdge {
            from: "src/b.rs".to_string(),
            to: "src/a.rs".to_string(),
            kind: "import".to_string(),
            weight: 1.0,
        });

        let pg = CodeGraph::open_in_memory().unwrap();
        populate_from_project_index(&pg, &a).unwrap();
        let provider = GraphProvider::new(pg, "/test");
        let b = provider.materialize_project_index();

        // Files: inventory + every catalog field survive.
        let mut a_files: Vec<&String> = a.files.keys().collect();
        let mut b_files: Vec<&String> = b.files.keys().collect();
        a_files.sort();
        b_files.sort();
        assert_eq!(a_files, b_files, "file inventory must round-trip");
        for (path, fa) in &a.files {
            let fb = b.files.get(path).expect("file present after round trip");
            assert_eq!(fa.hash, fb.hash, "hash {path}");
            assert_eq!(fa.language, fb.language, "language {path}");
            assert_eq!(fa.line_count, fb.line_count, "line_count {path}");
            assert_eq!(fa.token_count, fb.token_count, "token_count {path}");
            assert_eq!(fa.exports, fb.exports, "exports {path}");
            assert_eq!(fa.summary, fb.summary, "summary {path}");
        }

        // Symbols: keys + spans + the metadata-carried kind/export flag survive.
        let mut a_syms: Vec<&String> = a.symbols.keys().collect();
        let mut b_syms: Vec<&String> = b.symbols.keys().collect();
        a_syms.sort();
        b_syms.sort();
        assert_eq!(a_syms, b_syms, "symbol table must round-trip");
        for (key, sa) in &a.symbols {
            let sb = b.symbols.get(key).expect("symbol present after round trip");
            assert_eq!(sa.name, sb.name, "name {key}");
            assert_eq!(sa.file, sb.file, "file {key}");
            assert_eq!(sa.kind, sb.kind, "kind {key}");
            assert_eq!(sa.start_line, sb.start_line, "start_line {key}");
            assert_eq!(sa.end_line, sb.end_line, "end_line {key}");
            assert_eq!(sa.is_exported, sb.is_exported, "is_exported {key}");
        }

        // Structural edges: the import edge survives (PG may enrich, never lose).
        assert!(
            b.edges
                .iter()
                .any(|e| e.from == "src/b.rs" && e.to == "src/a.rs" && e.kind == "import"),
            "import edge must round-trip; got {:?}",
            b.edges
        );
    }
}
