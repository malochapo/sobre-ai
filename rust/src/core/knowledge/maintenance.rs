//! Maintenance for the on-disk knowledge stores.
//!
//! A store at `<data_dir>/knowledge/<hash>/` is keyed to a `project_root`. When
//! that root is deleted (a removed git worktree, a thrown-away temp project) the
//! store is never written again: its per-store eviction cap cannot self-heal
//! (the memory lifecycle only runs on write). This module finds those orphaned
//! stores so `sobre doctor` can list them. Nothing deletes one: a missing root
//! can also mean a moved project or an unmounted drive.

use std::path::{Path, PathBuf};

use super::ProjectKnowledge;

/// A knowledge store whose recorded `project_root` no longer exists on disk.
#[derive(Debug, Clone)]
pub struct OrphanedStore {
    pub dir: PathBuf,
    pub size_bytes: u64,
}

/// Scans every knowledge store under the data dir and returns the orphaned
/// ones.
#[must_use]
pub fn find_orphaned_stores() -> Vec<OrphanedStore> {
    match crate::core::data_dir::sobre_data_dir() {
        Ok(data_dir) => find_orphaned_stores_in(&data_dir),
        Err(_) => Vec::new(),
    }
}

/// Detects orphaned stores under an explicit `data_dir`.
///
/// A store is orphaned when its `project_root` is non-empty and does not
/// exist on disk. A store with an empty root (which cannot be checked), a live
/// root, or a `knowledge.json` that does not parse is never listed.
fn find_orphaned_stores_in(data_dir: &Path) -> Vec<OrphanedStore> {
    let knowledge_dir = data_dir.join("knowledge");
    let Ok(entries) = std::fs::read_dir(&knowledge_dir) else {
        return Vec::new();
    };

    let mut orphans = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(dir.join("knowledge.json")) else {
            continue;
        };
        let Ok(store) = serde_json::from_str::<ProjectKnowledge>(&content) else {
            continue;
        };

        let root = store.project_root.trim();
        // An empty root cannot be checked, so the store is kept.
        if root.is_empty() {
            continue;
        }
        // Live project: keep.
        if Path::new(root).exists() {
            continue;
        }

        let size_bytes = dir_size(&dir);
        orphans.push(OrphanedStore { dir, size_bytes });
    }
    orphans
}

/// Recursively sum the size of every regular file under `dir`. Best-effort:
/// unreadable entries contribute zero.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total = total.saturating_add(dir_size(&path));
        } else if let Ok(meta) = path.metadata() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_store(data_dir: &Path, hash: &str, project_root: &str) {
        let dir = data_dir.join("knowledge").join(hash);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ProjectKnowledge::new(project_root);
        let json = serde_json::to_string(&store).unwrap();
        std::fs::write(dir.join("knowledge.json"), json).unwrap();
    }

    #[test]
    fn detects_only_missing_root_stores() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();

        // Live root: the temp dir itself exists.
        let live_root = data_dir.to_string_lossy().to_string();
        write_store(data_dir, "live0000000000000", &live_root);
        // Empty root: cannot be checked — must never be flagged.
        write_store(data_dir, "empty000000000000", "");
        // Missing root: a path that does not exist — the orphan.
        let missing_root = data_dir
            .join("deleted-worktree")
            .to_string_lossy()
            .to_string();
        write_store(data_dir, "dead00000000000000", &missing_root);

        let orphans = find_orphaned_stores_in(data_dir);
        assert_eq!(orphans.len(), 1, "only the missing-root store is an orphan");
        assert!(orphans[0].dir.ends_with("dead00000000000000"));
        assert!(orphans[0].size_bytes > 0, "orphan size should be measured");
    }
}
