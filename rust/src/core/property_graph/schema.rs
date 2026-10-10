//! Database schema initialization for the code graph.

use rusqlite::Connection;

pub(super) fn initialize(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA cache_size  = -8000;
        PRAGMA mmap_size   = 268435456;
        PRAGMA temp_store  = MEMORY;

        CREATE TABLE IF NOT EXISTS paths (
            id   INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS nodes (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            kind       TEXT NOT NULL,
            name       TEXT NOT NULL,
            file_id    INTEGER NOT NULL REFERENCES paths(id),
            line_start INTEGER,
            line_end   INTEGER,
            metadata   TEXT,
            UNIQUE(kind, name, file_id)
        );

        CREATE INDEX IF NOT EXISTS idx_nodes_file
            ON nodes(file_id);
        CREATE INDEX IF NOT EXISTS idx_nodes_name
            ON nodes(name);
        CREATE INDEX IF NOT EXISTS idx_nodes_kind
            ON nodes(kind);
        CREATE INDEX IF NOT EXISTS idx_nodes_kind_file
            ON nodes(kind, file_id);

        CREATE TABLE IF NOT EXISTS edges (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            kind      TEXT NOT NULL,
            metadata  TEXT,
            UNIQUE(source_id, target_id, kind)
        );

        CREATE INDEX IF NOT EXISTS idx_edges_source
            ON edges(source_id);
        CREATE INDEX IF NOT EXISTS idx_edges_target
            ON edges(target_id);
        CREATE INDEX IF NOT EXISTS idx_edges_kind
            ON edges(kind);
        CREATE INDEX IF NOT EXISTS idx_edges_source_kind
            ON edges(source_id, kind);
        CREATE INDEX IF NOT EXISTS idx_edges_target_kind
            ON edges(target_id, kind);

        CREATE TABLE IF NOT EXISTS file_catalog (
            file_id     INTEGER PRIMARY KEY REFERENCES paths(id),
            hash        TEXT NOT NULL,
            language    TEXT NOT NULL DEFAULT '',
            line_count  INTEGER NOT NULL DEFAULT 0,
            token_count INTEGER NOT NULL DEFAULT 0,
            exports     TEXT NOT NULL DEFAULT '[]',
            summary     TEXT NOT NULL DEFAULT ''
        );
        ",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_creates_tables() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        assert!(tables.contains(&"nodes".to_string()));
        assert!(tables.contains(&"edges".to_string()));
        assert!(tables.contains(&"file_catalog".to_string()));
        assert!(tables.contains(&"paths".to_string()));
    }
}
