//! SQLite-backed file-path flyweights for the PropertyGraph.

use rusqlite::{Connection, params};

/// Compact graph-local handle for one canonical path in the `paths` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct FileId(i64);

impl FileId {
    pub(crate) fn raw(self) -> i64 {
        self.0
    }
}

/// Resolve or create the stable ID for `path` within this graph database.
pub(crate) fn intern(conn: &Connection, path: &str) -> anyhow::Result<FileId> {
    conn.execute(
        "INSERT INTO paths (path) VALUES (?1) ON CONFLICT(path) DO NOTHING",
        params![path],
    )?;
    let id = conn.query_row(
        "SELECT id FROM paths WHERE path = ?1",
        params![path],
        |row| row.get(0),
    )?;
    Ok(FileId(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::property_graph::CodeGraph;

    #[test]
    fn intern_deduplicates() {
        let graph = CodeGraph::open_in_memory().unwrap();
        let first = intern(graph.connection(), "src/lib.rs").unwrap();
        let second = intern(graph.connection(), "src/lib.rs").unwrap();
        assert_eq!(first, second);
    }
}
