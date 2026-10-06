//! Persisted project health (`health.json`) + stale-guarded background refresh.
//!
//! The engine computes the [`NavigabilityScore`] once per index build and stores
//! it next to the graph index. Session-start and other surfaces then *read* it
//! (no re-parse), realizing the plan's "compute once, fan-out everywhere".
//!
//! The refresh is gated by a fingerprint of the indexed source set, so a touch
//! with no byte change never triggers a recompute (mirrors the index's own
//! content-hash reuse).

use super::scan::scan_project;
use super::score::NavigabilityScore;
use crate::core::graph_index::ProjectIndex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const HEALTH_FILE: &str = "health.json";

/// Hotspots retained in the persisted score.
const TOP_HOTSPOTS: usize = 10;

/// Project health persisted next to the graph index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedHealth {
    /// Fingerprint of the indexed source set; recompute only when it changes.
    pub fingerprint: String,
    pub threshold: u32,
    pub naming_count: usize,
    pub score: NavigabilityScore,
}

fn health_path(root: &str) -> Option<PathBuf> {
    Some(ProjectIndex::index_dir(root)?.join(HEALTH_FILE))
}

/// Load the persisted health for `root`, or `None` when absent/unreadable.
pub fn load(root: &str) -> Option<PersistedHealth> {
    let bytes = std::fs::read(health_path(root)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Fingerprint of the indexed file set (path + content hash). Stable + cheap so
/// it can gate the recompute without re-reading any source.
fn fingerprint(index: &ProjectIndex) -> String {
    let mut entries: Vec<String> = index
        .files
        .values()
        .map(|f| format!("{}:{}", f.path, f.hash))
        .collect();
    entries.sort();
    blake3::hash(entries.join("\n").as_bytes())
        .to_hex()
        .to_string()
}

/// Recompute + persist health only when the indexed source set changed. Safe to
/// call from the background indexer (off the hot path); never panics.
pub fn refresh_if_stale(root: &str, index: &ProjectIndex) {
    let Some(path) = health_path(root) else {
        return;
    };
    let fp = fingerprint(index);
    if load(root).is_some_and(|existing| existing.fingerprint == fp) {
        return;
    }

    let threshold = crate::core::code_health::COGNITIVE_THRESHOLD;
    let health = scan_project(Path::new(root), threshold, None, TOP_HOTSPOTS);

    // Phase 3: fan the (top-N) hotspots out across BM25 / property graph /
    // knowledge as a replace-source, so health is queryable + cross-linked and
    // resolved hotspots are pruned. Done before the score is moved below.
    super::fabric::apply(root, &health);

    let persisted = PersistedHealth {
        fingerprint: fp,
        threshold,
        naming_count: health.naming_count,
        score: health.score,
    };

    if let Ok(json) = serde_json::to_vec_pretty(&persisted) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }
}
