//! MCP-server-safe storage maintenance.
//!
//! Unlike the interactive `sobre doctor --fix` (which reports what it
//! pruned), these routines are silent (tracing only) so they can run inside the MCP
//! server without corrupting the stdio protocol. They remove accumulated
//! quarantined BM25 indexes.

use std::path::PathBuf;

const QUARANTINED_FILES: &[&str] = &["bm25_index.bin.zst.quarantined"];

/// Remove accumulated quarantined BM25 index files. These are dead weight: an
/// index is only quarantined when it failed a load/size check and was replaced.
fn prune_quarantined_bm25() -> (u32, u64) {
    let mut removed = 0u32;
    let mut freed = 0u64;
    let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() else {
        return (removed, freed);
    };
    let vectors_dir = data_dir.join("vectors");
    let Ok(entries) = std::fs::read_dir(&vectors_dir) else {
        return (removed, freed);
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        for q_name in QUARANTINED_FILES {
            let q: PathBuf = dir.join(q_name);
            if q.exists() {
                if let Ok(meta) = std::fs::metadata(&q) {
                    freed = freed.saturating_add(meta.len());
                }
                if std::fs::remove_file(&q).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    (removed, freed)
}

/// Run a silent maintenance pass: prune quarantined BM25 indexes. Safe to
/// call from the MCP server.
pub fn run_quiet() {
    let (quarantined_removed, bytes_freed) = prune_quarantined_bm25();
    if quarantined_removed > 0 {
        tracing::info!(
            "storage maintenance: pruned {quarantined_removed} quarantined BM25 index file(s) \
             (freed {bytes_freed} bytes)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_removes_quarantined_files() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        crate::test_env::set_var("SOBRE_DATA_DIR", tmp.path());

        let idx_dir = tmp.path().join("vectors").join("proj_abc");
        std::fs::create_dir_all(&idx_dir).unwrap();
        std::fs::write(idx_dir.join("bm25_index.bin.zst.quarantined"), b"dead").unwrap();
        std::fs::write(idx_dir.join("bm25_index.bin.zst"), b"live").unwrap();

        let (removed, freed) = prune_quarantined_bm25();
        assert_eq!(removed, 1);
        assert!(freed >= 4);
        assert!(!idx_dir.join("bm25_index.bin.zst.quarantined").exists());
        assert!(
            idx_dir.join("bm25_index.bin.zst").exists(),
            "live index must be preserved"
        );

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }
}
