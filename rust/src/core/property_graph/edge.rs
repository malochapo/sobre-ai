//! Edge types and CRUD operations for graph edges.

use rusqlite::{Connection, params};

/// What a file edge says. The first three are code references (one file needs
/// the other); the last two are heuristics (see `queries`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    Imports,
    /// Module declaration or re-export (`mod x;`, `__init__.py`, barrels).
    Module,
    /// A type or extension-method use resolved to its defining file.
    TypeRef,
    /// Files changed together in recent git history.
    Cochange,
    /// Files in the same directory (connects files nothing else reaches).
    Sibling,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Imports => "imports",
            Self::Module => "module",
            Self::TypeRef => "type_ref",
            Self::Cochange => "cochange",
            Self::Sibling => "sibling",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "imports" => Some(Self::Imports),
            "module" => Some(Self::Module),
            "type_ref" => Some(Self::TypeRef),
            "cochange" => Some(Self::Cochange),
            "sibling" => Some(Self::Sibling),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub id: Option<i64>,
    pub source_id: i64,
    pub target_id: i64,
    pub kind: EdgeKind,
    pub metadata: Option<String>,
}

impl Edge {
    pub fn new(source_id: i64, target_id: i64, kind: EdgeKind) -> Self {
        Self {
            id: None,
            source_id,
            target_id,
            kind,
            metadata: None,
        }
    }

    #[cfg(test)]
    pub fn with_metadata(mut self, meta: &str) -> Self {
        self.metadata = Some(meta.to_string());
        self
    }
}

pub(super) fn upsert(conn: &Connection, edge: &Edge) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO edges (source_id, target_id, kind, metadata)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(source_id, target_id, kind) DO UPDATE SET
            metadata = excluded.metadata",
        params![
            edge.source_id,
            edge.target_id,
            edge.kind.as_str(),
            edge.metadata,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
pub(super) fn from_node(conn: &Connection, node_id: i64) -> anyhow::Result<Vec<Edge>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_id, target_id, kind, metadata
         FROM edges WHERE source_id = ?1",
    )?;
    let edges = stmt
        .query_map(params![node_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, String>(3)?,
                row.get(4)?,
            ))
        })?
        .filter_map(std::result::Result::ok)
        .filter_map(|(id, source_id, target_id, kind, metadata)| {
            Some(Edge {
                id: Some(id),
                source_id,
                target_id,
                kind: EdgeKind::parse(&kind)?,
                metadata,
            })
        })
        .collect();
    Ok(edges)
}

#[cfg(test)]
pub(super) fn to_node(conn: &Connection, node_id: i64) -> anyhow::Result<Vec<Edge>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_id, target_id, kind, metadata
         FROM edges WHERE target_id = ?1",
    )?;
    let edges = stmt
        .query_map(params![node_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, String>(3)?,
                row.get(4)?,
            ))
        })?
        .filter_map(std::result::Result::ok)
        .filter_map(|(id, source_id, target_id, kind, metadata)| {
            Some(Edge {
                id: Some(id),
                source_id,
                target_id,
                kind: EdgeKind::parse(&kind)?,
                metadata,
            })
        })
        .collect();
    Ok(edges)
}

pub(super) fn count(conn: &Connection) -> anyhow::Result<usize> {
    let c: i64 = conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;
    Ok(c as usize)
}
