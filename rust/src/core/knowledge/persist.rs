use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::ranking::hash_project_root;
use super::types::ProjectKnowledge;

fn knowledge_dir(project_hash: &str) -> Result<PathBuf, String> {
    Ok(crate::core::data_dir::sobre_data_dir()?
        .join("knowledge")
        .join(project_hash))
}

/// Per-project-hash mutex registry. Serializes the read-modify-write cycle of
/// `mutate_locked` so concurrent `remember` calls within a single process (e.g.
/// parallel MCP tool calls) cannot clobber each other (issue #326). The outer
/// map lock is held only briefly to clone the inner `Arc`; the inner lock is
/// held across the load → mutate → save cycle.
fn knowledge_lock(project_hash: &str) -> Arc<Mutex<()>> {
    static KNOWLEDGE_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = KNOWLEDGE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .entry(project_hash.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Acquires an exclusive, cross-process advisory lock for a project's
/// knowledge store. The returned file handle holds the lock until it is
/// dropped; the OS releases it automatically if the process exits (even on
/// crash), so there are no stale locks. This serializes the read-modify-write
/// cycle across *separate processes* (parallel CLI invocations, CLI + daemon +
/// MCP server), complementing the in-process mutex (issue #326).
fn acquire_file_lock(dir: &Path) -> Option<std::fs::File> {
    use fs2::FileExt;
    let lock_path = dir.join(".knowledge.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600));
    }
    // Blocks until every other process holding the lock releases it. A failure
    // here (unsupported FS, etc.) degrades to the in-process lock only.
    file.lock_exclusive().ok()?;
    Some(file)
}

/// Atomically writes `json` to `path` by writing to a unique temp file in the
/// same directory and renaming it into place. `rename` is atomic on every
/// supported platform (and replaces the target on Windows), so readers and
/// concurrent writers never observe a half-written file — preventing the
/// trailing-garbage JSON corruption reported in issue #326.
fn write_json_atomic(dir: &Path, path: &Path, json: &str) -> Result<(), String> {
    let unique = format!(
        "knowledge.json.tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let tmp = dir.join(unique);
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}

impl ProjectKnowledge {
    pub fn save(&self) -> Result<(), String> {
        let dir = knowledge_dir(&self.project_hash)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }

        let path = dir.join("knowledge.json");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_json_atomic(&dir, &path, &json)?;
        Ok(())
    }

    /// Runs `f` while holding this project's locks — the in-process per-hash
    /// mutex *and* the cross-process advisory file lock — without loading or
    /// saving the knowledge JSON itself. [`mutate_locked`](Self::mutate_locked)
    /// is built on this, and side-car stores that must stay consistent with the
    /// facts (today: the embedding index) call it directly so their
    /// read-modify-write is serialized against parallel
    /// `remember`/`remove`/`reindex`. That side-car write used to run lock-free,
    /// so concurrent callers clobbered each other's embeddings and pruned
    /// just-stored vectors, degrading semantic recall (issue #412, a #326
    /// follow-up).
    pub(crate) fn with_project_lock<T>(project_root: &str, f: impl FnOnce() -> T) -> T {
        let hash = hash_project_root(project_root);
        let lock = knowledge_lock(&hash);
        let _guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Cross-process lock: create the dir up front so the lock file has a
        // home, then block until any other process releases it. Held for the
        // whole critical section via `_file_lock`'s lifetime.
        let _file_lock = match knowledge_dir(&hash) {
            Ok(dir) => {
                let _ = std::fs::create_dir_all(&dir);
                acquire_file_lock(&dir)
            }
            Err(_) => None,
        };

        f()
    }

    /// Runs a read-modify-write cycle under `with_project_lock`, then saves
    /// atomically. The knowledge is (re)loaded *inside* the lock so
    /// the closure always operates on the latest on-disk state; this is what
    /// prevents lost updates when several `remember` calls run in parallel —
    /// whether as threads in one process (parallel MCP calls) or as separate
    /// processes (parallel CLI invocations, CLI + daemon + MCP server) — see
    /// issue #326. Returns the persisted knowledge plus the closure's return
    /// value so the caller can build a response from the committed state.
    pub fn mutate_locked<T>(
        project_root: &str,
        f: impl FnOnce(&mut Self) -> T,
    ) -> Result<(Self, T), String> {
        Self::with_project_lock(project_root, || {
            let mut knowledge = Self::load_or_create(project_root);
            let out = f(&mut knowledge);
            knowledge.save()?;
            Ok((knowledge, out))
        })
    }

    pub fn load(project_root: &str) -> Option<Self> {
        let hash = hash_project_root(project_root);
        let dir = knowledge_dir(&hash).ok()?;
        let path = dir.join("knowledge.json");

        if let Ok(content) = std::fs::read_to_string(&path) {
            let size = content.len();
            if size > 1_000_000 {
                tracing::warn!(
                    "knowledge.json is large ({:.1} MB) — recall may be slow. \
                     Consider running sobre_knowledge(action=\"consolidate\") to compact it.",
                    size as f64 / 1_048_576.0,
                );
            }
            if let Ok(k) = serde_json::from_str::<Self>(&content) {
                return Some(k);
            }
        }

        None
    }

    pub fn load_or_create(project_root: &str) -> Self {
        Self::load(project_root).unwrap_or_else(|| Self::new(project_root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;

    #[test]
    fn file_lock_is_exclusive_across_handles() {
        // flock-style locks are tied to the open file description, so two
        // independent `open()`s in the same process behave like two separate
        // processes: while the first holds the exclusive lock, the second must
        // fail to acquire it. This validates the cross-process guarantee that
        // protects parallel CLI writes (issue #326).
        let dir = tempfile::tempdir().unwrap();
        let held = acquire_file_lock(dir.path()).expect("first lock must succeed");

        let second = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.path().join(".knowledge.lock"))
            .unwrap();
        assert!(
            second.try_lock_exclusive().is_err(),
            "a second handle must not acquire the lock while it is held"
        );

        drop(held);
        // `close()` releases the flock synchronously, so the lock IS free here.
        // Under heavy parallel test load, however, a single non-blocking
        // `try_lock_exclusive()` can momentarily observe `EWOULDBLOCK` from
        // scheduling jitter. A short bounded retry removes that flake without
        // weakening the guarantee: the lock must become acquirable again.
        let mut reacquired = false;
        for _ in 0..50 {
            if second.try_lock_exclusive().is_ok() {
                reacquired = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            reacquired,
            "lock must be acquirable within 500ms of release"
        );
    }

    #[test]
    fn write_json_atomic_leaves_valid_file_and_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("knowledge.json");
        write_json_atomic(dir.path(), &path, "{\"ok\":true}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"ok\":true}");
        let leftover = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains(".tmp."));
        assert!(!leftover, "no temp file should remain");
    }
}
