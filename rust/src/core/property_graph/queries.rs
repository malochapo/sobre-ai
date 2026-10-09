//! Graph traversal queries: impact analysis and related files.
//!
//! Two classes of edges. Code references (imports, module declarations and
//! re-exports, type uses) say one file needs another: impact follows only
//! these. Heuristics (files changed together in git, files in the same
//! directory) say two files tend to be read together: only `related_files`
//! adds them.

use std::collections::HashMap;

use rusqlite::{Connection, params};

#[derive(Debug, Clone)]
pub struct ImpactResult {
    pub affected_files: Vec<String>,
    pub max_depth_reached: usize,
    pub edges_traversed: usize,
}

/// Edge kinds that are code references: one file needs the other.
const REFERENCE_EDGE_KINDS: &str = "'imports','module','type_ref'";

/// References plus the heuristics, for `related_files` only.
const RELATED_EDGE_KINDS: &str = "'imports','module','type_ref','cochange','sibling'";

/// Weight multiplier per edge kind, for impact attenuation and related ranking.
pub(crate) fn edge_weight(kind: &str) -> f64 {
    match kind {
        "imports" => 1.0,
        "module" => 0.6,
        "type_ref" => 0.5,
        "cochange" => 0.35,
        "sibling" => 0.25,
        _ => 0.1,
    }
}

/// The files that depend on `file_path` through reverse reference edges, at
/// most `max_depth` hops away, sorted. Edge weights attenuate along a path (a
/// type use carries less impact than an import): a file is affected when some
/// path of at most `max_depth` hops keeps a weight product of at least 0.1.
/// Each hop relaxes every file whose best weight just improved, so the answer
/// does not depend on the order edges are visited in.
pub(super) fn impact_analysis(
    conn: &Connection,
    file_path: &str,
    max_depth: usize,
) -> anyhow::Result<ImpactResult> {
    // Graph node keys use canonical `/` separators (see the builder walk);
    // accept native Windows input too.
    let file_path = file_path.replace('\\', "/");
    let file_path = file_path.as_str();
    let reverse_graph = build_weighted_reverse_graph(conn)?;
    const PROPAGATION_THRESHOLD: f64 = 0.1;

    // Best weight found so far for each reached file.
    let mut best: HashMap<String, f64> = HashMap::from([(file_path.to_string(), 1.0)]);
    let mut frontier: HashMap<String, f64> = best.clone();
    let mut max_depth_reached = 0;
    let mut edges_traversed = 0;

    for depth in 1..=max_depth {
        let mut next: HashMap<String, f64> = HashMap::new();
        for (current, weight) in &frontier {
            let Some(dependents) = reverse_graph.get(current) else {
                continue;
            };
            for (dep, ew) in dependents {
                edges_traversed += 1;
                let propagated = weight * ew;
                if propagated < PROPAGATION_THRESHOLD {
                    continue;
                }
                let known = best.get(dep).copied();
                if known.is_some_and(|w| w >= propagated) {
                    continue;
                }
                if known.is_none() {
                    max_depth_reached = depth;
                }
                best.insert(dep.clone(), propagated);
                let slot = next.entry(dep.clone()).or_insert(0.0);
                *slot = slot.max(propagated);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }

    best.remove(file_path);
    let mut affected_files: Vec<String> = best.into_keys().collect();
    affected_files.sort();

    Ok(ImpactResult {
        affected_files,
        max_depth_reached,
        edges_traversed,
    })
}

/// Related files for a given path: direct neighbors via a reference or a heuristic,
/// sorted by edge weight (strongest relationship first). Returns (path, weight) pairs.
pub(crate) fn related_files(
    conn: &Connection,
    file_path: &str,
    limit: usize,
) -> anyhow::Result<Vec<(String, f64)>> {
    let sql = format!(
        "SELECT p_other.path, e.kind
         FROM edges e
         JOIN nodes n_self ON (e.source_id = n_self.id OR e.target_id = n_self.id)
         JOIN nodes n_other ON (
             (e.source_id = n_other.id AND e.target_id = n_self.id)
             OR (e.target_id = n_other.id AND e.source_id = n_self.id)
         )
         JOIN paths p_self ON p_self.id = n_self.file_id
         JOIN paths p_other ON p_other.id = n_other.file_id
         WHERE p_self.path = ?1
           AND p_other.path != ?1
           AND e.kind IN ({RELATED_EDGE_KINDS})"
    );
    let mut stmt = conn.prepare(&sql)?;

    let mut scores: HashMap<String, f64> = HashMap::new();
    let rows = stmt.query_map(params![file_path], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (path, kind) = row?;
        *scores.entry(path).or_default() += edge_weight(&kind);
    }

    // Ties (same kinds of edges) are common and the map has no order: break
    // them by path so the list, and the cut at `limit`, are stable.
    let mut results: Vec<(String, f64)> = scores.into_iter().collect();
    results.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    results.truncate(limit);
    Ok(results)
}

fn build_weighted_reverse_graph(
    conn: &Connection,
) -> anyhow::Result<HashMap<String, Vec<(String, f64)>>> {
    let sql = format!(
        "SELECT p_tgt.path, p_src.path, e.kind
         FROM edges e
         JOIN nodes n_src ON e.source_id = n_src.id
         JOIN nodes n_tgt ON e.target_id = n_tgt.id
         JOIN paths p_src ON p_src.id = n_src.file_id
         JOIN paths p_tgt ON p_tgt.id = n_tgt.file_id
         WHERE e.kind IN ({REFERENCE_EDGE_KINDS})
           AND p_src.path != p_tgt.path"
    );
    let mut stmt = conn.prepare(&sql)?;

    let mut graph: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    for row in rows {
        let (target, source, kind) = row?;
        let w = edge_weight(&kind);
        let entry = graph
            .entry(target)
            .or_default()
            .entry(source)
            .or_insert(0.0);
        if w > *entry {
            *entry = w;
        }
    }

    Ok(graph
        .into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect())
}
