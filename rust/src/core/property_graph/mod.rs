//! Property Graph Engine — SQLite-backed code knowledge graph.
//!
//! Stores nodes (File, Symbol, Module) and edges (imports, calls, defines,
//! exports) extracted by `deep_queries` + `import_resolver`.  Provides
//! efficient traversal queries for impact analysis, architecture discovery,
//! and graph-driven context loading.

mod cross_source;
mod edge;
pub mod file_catalog;
mod meta;
mod node;
mod path_id;
mod queries;
mod schema;
mod sync;

pub use edge::{Edge, EdgeKind};
pub use file_catalog::FileCatalogEntry;
pub use meta::{PropertyGraphMetaV1, load_meta, meta_path, write_meta};
pub use node::{Node, NodeKind};
pub use queries::{DependencyChain, ImpactResult, edge_weight, related_files};
#[cfg(test)]
pub(crate) use sync::populate_from_project_index;
pub use sync::{mirror_index, parse_symbol_metadata};

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Resolve the directory for graph.db and graph.meta.json.
///
/// Uses `$SOBRE_DATA_DIR/graphs/<project_hash>/` (consistent with
/// `ProjectIndex::index_dir`).  Falls back to `<project>/.sobre/`
/// only when the global data directory cannot be resolved.
pub fn graph_dir(project_root: &str) -> PathBuf {
    if let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() {
        let normalized = crate::core::index_paths::normalize_project_root(project_root);
        let hash = crate::core::project_hash::hash_project_root(&normalized);
        data_dir.join("graphs").join(hash)
    } else {
        Path::new(project_root).join(".sobre")
    }
}

/// Property-graph engine generation, stamped into `graph.meta.json` on every
/// build. Bump whenever edge extraction or the storage layout changes: a graph
/// stamped with an older engine version is rebuilt on the next query instead
/// of being served without the new edges.
pub const GRAPH_ENGINE_VERSION: u32 = 5;

/// `true` when the persisted graph was built by an engine older than
/// [`GRAPH_ENGINE_VERSION`] — or has no readable meta (never built) — and must
/// therefore be rebuilt before its edges can be trusted. Callers pair this with
/// a node-count check: an empty graph is rebuilt regardless; a
/// non-empty-but-outdated graph is rebuilt by this gate.
pub fn engine_outdated(project_root: &str) -> bool {
    load_meta(project_root).is_none_or(|m| m.engine_version < GRAPH_ENGINE_VERSION)
}

/// Whether an open error is a transient SQLite lock (BUSY/LOCKED) that a brief
/// retry can clear — notably the `PRAGMA journal_mode=WAL` and initial-DDL races
/// that SQLite reports *without* invoking the busy handler (so `busy_timeout`
/// does not cover them).
fn is_transient_lock(err: &anyhow::Error) -> bool {
    use rusqlite::ErrorCode;
    if let Some(rusqlite::Error::SqliteFailure(e, _)) = err.downcast_ref::<rusqlite::Error>()
        && matches!(e.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    {
        return true;
    }
    // The schema initializer may surface the same condition wrapped; match the
    // full error chain's text as a fallback.
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("database is locked") || msg.contains("table is locked")
}

pub struct CodeGraph {
    conn: Connection,
    db_path: PathBuf,
}

impl CodeGraph {
    pub fn open(project_root: &str) -> anyhow::Result<Self> {
        let db_dir = graph_dir(project_root);
        std::fs::create_dir_all(&db_dir)?;
        let db_path = db_dir.join("graph.db");

        // Concurrent opens race on `PRAGMA journal_mode=WAL` and the initial DDL,
        // which SQLite reports as SQLITE_BUSY without invoking the busy handler
        // (`busy_timeout` therefore does not apply). Two sessions can open the
        // same db at once; retry briefly so neither silently loses its writes. Once WAL is recorded in the file header, opens stop racing.
        const MAX_ATTEMPTS: u32 = 12;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match Self::try_open(&db_path) {
                Ok(graph) => return Ok(graph),
                Err(e) if attempt < MAX_ATTEMPTS && is_transient_lock(&e) => {
                    std::thread::sleep(std::time::Duration::from_millis(40 * u64::from(attempt)));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// One open attempt: connect, register the busy handler, ensure the schema.
    fn try_open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        schema::initialize(&conn)?;
        Ok(Self {
            conn,
            db_path: db_path.to_path_buf(),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::initialize(&conn)?;
        Ok(Self {
            conn,
            db_path: PathBuf::from(":memory:"),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn upsert_node(&self, node: &Node) -> anyhow::Result<i64> {
        node::upsert(&self.conn, node)
    }

    pub fn upsert_edge(&self, edge: &Edge) -> anyhow::Result<()> {
        edge::upsert(&self.conn, edge)
    }

    pub fn get_node_by_path(&self, file_path: &str) -> anyhow::Result<Option<Node>> {
        node::get_by_path(&self.conn, file_path)
    }

    pub fn get_node_by_symbol(&self, name: &str, file_path: &str) -> anyhow::Result<Option<Node>> {
        node::get_by_symbol(&self.conn, name, file_path)
    }

    #[cfg(test)]
    pub fn edges_from(&self, node_id: i64) -> anyhow::Result<Vec<Edge>> {
        edge::from_node(&self.conn, node_id)
    }

    #[cfg(test)]
    pub fn edges_to(&self, node_id: i64) -> anyhow::Result<Vec<Edge>> {
        edge::to_node(&self.conn, node_id)
    }

    pub fn dependents(&self, file_path: &str) -> anyhow::Result<Vec<String>> {
        queries::dependents(&self.conn, file_path)
    }

    pub fn impact_analysis(
        &self,
        file_path: &str,
        max_depth: usize,
    ) -> anyhow::Result<ImpactResult> {
        queries::impact_analysis(&self.conn, file_path, max_depth)
    }

    pub fn dependency_chain(
        &self,
        from: &str,
        to: &str,
    ) -> anyhow::Result<Option<DependencyChain>> {
        queries::dependency_chain(&self.conn, from, to)
    }

    pub fn related_files(
        &self,
        file_path: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(String, f64)>> {
        queries::related_files(&self.conn, file_path, limit)
    }

    pub fn node_count(&self) -> anyhow::Result<usize> {
        node::count(&self.conn)
    }

    pub fn edge_count(&self) -> anyhow::Result<usize> {
        edge::count(&self.conn)
    }

    /// Persist a cross-source edge (code file ↔ external source URI) into the
    /// dedicated `cross_source_edges` table. Keeps the higher weight on conflict
    /// so repeated provider ingests don't downgrade an established link.
    pub fn upsert_cross_source_edge(
        &self,
        from: &str,
        to: &str,
        kind: &str,
        weight: f32,
    ) -> anyhow::Result<()> {
        cross_source::upsert(&self.conn, from, to, kind, weight)
    }

    /// All cross-source edges as `IndexEdge`s, ready for `cross_source_hints`.
    pub fn all_cross_source_edges(&self) -> Vec<crate::core::graph_index::IndexEdge> {
        cross_source::all(&self.conn).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn cross_source_edge_count(&self) -> anyhow::Result<usize> {
        cross_source::count(&self.conn)
    }

    /// Delete every cross-source edge of a given `kind`. Used by the code-health
    /// fabric to replace its `health_hotspot` edges on each pass so resolved
    /// hotspots never persist as stale hints. Returns the number removed.
    pub fn delete_cross_source_edges_by_kind(&self, kind: &str) -> anyhow::Result<usize> {
        cross_source::delete_by_kind(&self.conn, kind)
    }

    /// Clear only the code graph (nodes, edges, file catalog), preserving
    /// provider `cross_source_edges`. Used by the graph_index→PG mirror
    /// so rebuilding the code graph never drops lateral provider hints, which
    /// live in their own table and are repopulated on a separate ingest cycle.
    pub fn clear_code_graph(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "DELETE FROM edges; DELETE FROM nodes; DELETE FROM file_catalog; DELETE FROM paths;",
        )?;
        Ok(())
    }

    pub fn upsert_file_catalog(&self, entry: &FileCatalogEntry) -> anyhow::Result<()> {
        file_catalog::upsert(&self.conn, entry)
    }

    pub fn get_file_catalog(&self, path: &str) -> anyhow::Result<Option<FileCatalogEntry>> {
        file_catalog::get(&self.conn, path)
    }

    pub fn file_catalog_count(&self) -> anyhow::Result<usize> {
        file_catalog::count(&self.conn)
    }

    pub fn file_catalog_paths(&self) -> anyhow::Result<Vec<String>> {
        file_catalog::all_paths(&self.conn)
    }

    pub fn find_symbols(
        &self,
        name: &str,
        file_filter: Option<&str>,
        kind_filter: Option<&str>,
    ) -> anyhow::Result<Vec<Node>> {
        node::find_symbols(&self.conn, name, file_filter, kind_filter)
    }

    /// Files that define a symbol named exactly `name` (symbol-name
    /// `sobre_graph action=impact` fallback). Backed by `node::resolve_symbol_def_files`.
    pub fn resolve_symbol_def_files(&self, name: &str) -> anyhow::Result<Vec<String>> {
        node::resolve_symbol_def_files(&self.conn, name)
    }

    pub fn symbol_count(&self) -> anyhow::Result<usize> {
        node::symbol_count(&self.conn)
    }

    /// Count of `file` nodes (accurate on both builder paths). Backed by
    /// `node::file_count`.
    pub fn file_node_count(&self) -> anyhow::Result<usize> {
        node::file_count(&self.conn)
    }

    /// Every symbol node with its line span (unfiltered). Backend for the
    /// call-graph symbol table after the `graph_index` teardown.
    pub fn all_symbols(&self) -> anyhow::Result<Vec<Node>> {
        node::all_symbols(&self.conn)
    }

    pub fn all_edges_flat(&self) -> anyhow::Result<Vec<(String, String, String, f64)>> {
        node::all_edges_flat(&self.conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::data_dir::test_env_lock;

    fn test_graph() -> CodeGraph {
        CodeGraph::open_in_memory().unwrap()
    }

    #[test]
    fn create_and_query_nodes() {
        let g = test_graph();

        let id = g.upsert_node(&Node::file("src/main.rs")).unwrap();
        assert!(id > 0);

        let found = g.get_node_by_path("src/main.rs").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().file_path, "src/main.rs");
    }

    #[test]
    fn create_and_query_edges() {
        let g = test_graph();

        let a = g.upsert_node(&Node::file("src/a.rs")).unwrap();
        let b = g.upsert_node(&Node::file("src/b.rs")).unwrap();

        g.upsert_edge(&Edge::new(a, b, EdgeKind::Imports)).unwrap();

        let from_a = g.edges_from(a).unwrap();
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_a[0].target_id, b);

        let to_b = g.edges_to(b).unwrap();
        assert_eq!(to_b.len(), 1);
        assert_eq!(to_b[0].source_id, a);
    }

    #[test]
    fn dependents_query() {
        let g = test_graph();

        let main = g.upsert_node(&Node::file("src/main.rs")).unwrap();
        let lib = g.upsert_node(&Node::file("src/lib.rs")).unwrap();
        let utils = g.upsert_node(&Node::file("src/utils.rs")).unwrap();

        g.upsert_edge(&Edge::new(main, lib, EdgeKind::Imports))
            .unwrap();
        g.upsert_edge(&Edge::new(utils, lib, EdgeKind::Imports))
            .unwrap();

        let deps = g.dependents("src/lib.rs").unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"src/main.rs".to_string()));
        assert!(deps.contains(&"src/utils.rs".to_string()));
    }

    #[test]
    #[allow(clippy::many_single_char_names)] // graph test nodes: a, b, c, d, e
    fn impact_analysis_depth() {
        let g = test_graph();

        let a = g.upsert_node(&Node::file("a.rs")).unwrap();
        let b = g.upsert_node(&Node::file("b.rs")).unwrap();
        let c = g.upsert_node(&Node::file("c.rs")).unwrap();
        let d = g.upsert_node(&Node::file("d.rs")).unwrap();

        g.upsert_edge(&Edge::new(b, a, EdgeKind::Imports)).unwrap();
        g.upsert_edge(&Edge::new(c, b, EdgeKind::Imports)).unwrap();
        g.upsert_edge(&Edge::new(d, c, EdgeKind::Imports)).unwrap();

        let impact = g.impact_analysis("a.rs", 2).unwrap();
        assert!(impact.affected_files.contains(&"b.rs".to_string()));
        assert!(impact.affected_files.contains(&"c.rs".to_string()));
        assert!(!impact.affected_files.contains(&"d.rs".to_string()));

        let deep = g.impact_analysis("a.rs", 10).unwrap();
        assert!(deep.affected_files.contains(&"d.rs".to_string()));
    }

    #[test]
    fn upsert_idempotent() {
        let g = test_graph();

        let id1 = g.upsert_node(&Node::file("src/main.rs")).unwrap();
        let id2 = g.upsert_node(&Node::file("src/main.rs")).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(g.node_count().unwrap(), 1);
    }

    #[test]
    fn dependency_chain_found() {
        let g = test_graph();

        let a = g.upsert_node(&Node::file("a.rs")).unwrap();
        let b = g.upsert_node(&Node::file("b.rs")).unwrap();
        let c = g.upsert_node(&Node::file("c.rs")).unwrap();

        g.upsert_edge(&Edge::new(a, b, EdgeKind::Imports)).unwrap();
        g.upsert_edge(&Edge::new(b, c, EdgeKind::Imports)).unwrap();

        let chain = g.dependency_chain("a.rs", "c.rs").unwrap();
        assert!(chain.is_some());
        let chain = chain.unwrap();
        assert_eq!(chain.path, vec!["a.rs", "b.rs", "c.rs"]);
    }

    #[test]
    fn counts() {
        let g = test_graph();
        assert_eq!(g.node_count().unwrap(), 0);
        assert_eq!(g.edge_count().unwrap(), 0);

        let a = g.upsert_node(&Node::file("a.rs")).unwrap();
        let b = g.upsert_node(&Node::file("b.rs")).unwrap();
        g.upsert_edge(&Edge::new(a, b, EdgeKind::Imports)).unwrap();

        assert_eq!(g.node_count().unwrap(), 2);
        assert_eq!(g.edge_count().unwrap(), 1);
    }

    #[test]
    fn dependents_follow_references_not_heuristics() {
        let g = test_graph();

        let a = g.upsert_node(&Node::file("src/a.rs")).unwrap();
        let b = g.upsert_node(&Node::file("src/b.rs")).unwrap();
        let c = g.upsert_node(&Node::file("src/c.rs")).unwrap();
        let peer = g.upsert_node(&Node::file("src/d.rs")).unwrap();

        g.upsert_edge(&Edge::new(b, a, EdgeKind::Imports)).unwrap();
        g.upsert_edge(&Edge::new(c, a, EdgeKind::TypeRef)).unwrap();
        g.upsert_edge(&Edge::new(peer, a, EdgeKind::Cochange))
            .unwrap();
        g.upsert_edge(&Edge::new(a, peer, EdgeKind::Sibling))
            .unwrap();

        assert_eq!(
            g.dependents("src/a.rs").unwrap(),
            vec!["src/b.rs".to_string(), "src/c.rs".to_string()]
        );
        assert!(
            g.dependency_chain("src/d.rs", "src/a.rs")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn impact_follows_references_transitively() {
        let g = test_graph();

        let a = g.upsert_node(&Node::file("a.rs")).unwrap();
        let b = g.upsert_node(&Node::file("b.rs")).unwrap();
        let c = g.upsert_node(&Node::file("c.rs")).unwrap();
        let peer = g.upsert_node(&Node::file("d.rs")).unwrap();

        g.upsert_edge(&Edge::new(b, a, EdgeKind::Imports)).unwrap();
        g.upsert_edge(&Edge::new(c, b, EdgeKind::Module)).unwrap();
        g.upsert_edge(&Edge::new(peer, a, EdgeKind::Cochange))
            .unwrap();

        let mut affected = g.impact_analysis("a.rs", 10).unwrap().affected_files;
        affected.sort();
        assert_eq!(affected, vec!["b.rs".to_string(), "c.rs".to_string()]);
    }

    #[test]
    fn related_files_adds_heuristics_ranked_by_weight() {
        let g = test_graph();

        let a = g.upsert_node(&Node::file("a.rs")).unwrap();
        let b = g.upsert_node(&Node::file("b.rs")).unwrap();
        let c = g.upsert_node(&Node::file("c.rs")).unwrap();
        let peer = g.upsert_node(&Node::file("d.rs")).unwrap();

        g.upsert_edge(&Edge::new(a, b, EdgeKind::Imports)).unwrap();
        g.upsert_edge(&Edge::new(a, c, EdgeKind::TypeRef)).unwrap();
        g.upsert_edge(&Edge::new(peer, a, EdgeKind::Cochange))
            .unwrap();

        let related = g.related_files("a.rs", 10).unwrap();
        let order: Vec<&str> = related.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(order, vec!["b.rs", "c.rs", "d.rs"]);
    }

    #[test]
    fn graph_dir_uses_data_dir_when_set() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("myproject");
        std::fs::create_dir_all(&project).unwrap();

        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let _guard = test_env_lock();
        crate::test_env::set_var("SOBRE_DATA_DIR", data_dir.to_str().unwrap());

        let dir = graph_dir(project.to_str().unwrap());
        assert!(dir.starts_with(&data_dir));
        assert!(dir.to_string_lossy().contains("graphs"));

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn graph_dir_returns_consistent_hash_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("hash_project");
        std::fs::create_dir_all(&project).unwrap();

        let data_dir = tmp.path().join("data2");
        std::fs::create_dir_all(&data_dir).unwrap();

        let _guard = test_env_lock();
        crate::test_env::set_var("SOBRE_DATA_DIR", data_dir.to_str().unwrap());

        let dir1 = graph_dir(project.to_str().unwrap());
        let dir2 = graph_dir(project.to_str().unwrap());
        assert_eq!(dir1, dir2, "graph_dir should be deterministic");
        assert!(dir1.to_string_lossy().contains("graphs"));

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn open_with_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("opentest");
        std::fs::create_dir_all(&project).unwrap();

        let data_dir = tmp.path().join("xdata");
        std::fs::create_dir_all(&data_dir).unwrap();

        let _guard = test_env_lock();
        crate::test_env::set_var("SOBRE_DATA_DIR", data_dir.to_str().unwrap());

        let g = CodeGraph::open(project.to_str().unwrap()).unwrap();
        assert!(g.db_path().starts_with(&data_dir));
        assert!(g.db_path().to_string_lossy().contains("graph.db"));

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn meta_path_uses_graph_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("metatest");
        std::fs::create_dir_all(&project).unwrap();

        let data_dir = tmp.path().join("mdata");
        std::fs::create_dir_all(&data_dir).unwrap();

        let _guard = test_env_lock();
        crate::test_env::set_var("SOBRE_DATA_DIR", data_dir.to_str().unwrap());

        let mp = meta::meta_path(project.to_str().unwrap());
        assert!(mp.starts_with(&data_dir));
        assert!(mp.to_string_lossy().contains("graph.meta.json"));

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn engine_outdated_flags_old_and_missing_meta() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let proj = tempfile::tempdir().unwrap();
        let root = proj.path().to_str().unwrap();

        // No meta on disk yet -> outdated (an unbuilt graph forces a build).
        assert!(engine_outdated(root), "missing meta must read as outdated");

        // Meta stamped with an older engine version -> outdated.
        let mut meta = PropertyGraphMetaV1 {
            built_at: "2026-01-01T00:00:00Z".to_string(),
            engine_version: 0,
            ..Default::default()
        };
        write_meta(root, &meta).unwrap();
        assert!(
            engine_outdated(root),
            "engine_version 0 must read as outdated"
        );

        // Meta stamped with the current engine -> up to date.
        meta.engine_version = GRAPH_ENGINE_VERSION;
        write_meta(root, &meta).unwrap();
        assert!(
            !engine_outdated(root),
            "current engine_version must read as up to date"
        );
    }
}
