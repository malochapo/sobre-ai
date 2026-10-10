use super::model::{GotchaStore, MAX_PENDING};
use std::path::{Path, PathBuf};

impl GotchaStore {
    /// Reads the project's store, or an empty one when there is none yet.
    /// `Err` (naming the file) when it exists but cannot be read or parsed.
    fn try_load(project_root: &str) -> Result<Self, String> {
        let hash = crate::core::project_hash::hash_project_root(project_root);
        let path = gotcha_path(&hash)?;
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new(&hash)),
            Err(e) => return Err(unreadable(&path, &e)),
        };
        let mut store: Self = serde_json::from_str(&content).map_err(|e| unreadable(&path, &e))?;
        store.apply_decay();
        store.cross_session_boost();
        // Keep persisted pending errors so a fail→fix that spans two
        // `sobre -c` processes still correlates, but drop expired ones
        // (15-min TTL) and keep the most recent MAX_PENDING, so a stale
        // file can never grow unbounded or resurrect old errors.
        store.pending_errors.retain(|p| !p.is_expired());
        if store.pending_errors.len() > MAX_PENDING {
            let excess = store.pending_errors.len() - MAX_PENDING;
            store.pending_errors.drain(0..excess);
        }
        Ok(store)
    }

    /// The store for read-only callers: one that cannot be read is logged
    /// and reads as empty.
    pub fn load(project_root: &str) -> Self {
        Self::try_load(project_root).unwrap_or_else(|e| {
            tracing::warn!("{e}");
            Self::new(&crate::core::project_hash::hash_project_root(project_root))
        })
    }

    /// Loads the project's store, runs `f` on it and saves it when `f`
    /// returns `true`, under the project's knowledge lock, so concurrent
    /// writers (threads or processes) never overwrite each other's gotchas.
    /// Returns what `f` returned. A store that exists but cannot be read is
    /// an error: `f` does not run and the file is left as it is.
    pub fn update(project_root: &str, f: impl FnOnce(&mut Self) -> bool) -> Result<bool, String> {
        crate::core::knowledge::ProjectKnowledge::with_project_lock(project_root, || {
            let mut store = Self::try_load(project_root)?;
            let changed = f(&mut store);
            if changed {
                store.save(project_root)?;
            }
            Ok(changed)
        })
    }

    pub fn save(&self, project_root: &str) -> Result<(), String> {
        let hash = crate::core::project_hash::hash_project_root(project_root);
        let path = gotcha_path(&hash)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }
}

fn gotcha_path(project_hash: &str) -> Result<PathBuf, String> {
    Ok(crate::core::data_dir::sobre_data_dir()?
        .join("knowledge")
        .join(project_hash)
        .join("gotchas.json"))
}

fn unreadable(path: &Path, e: &dyn std::fmt::Display) -> String {
    format!(
        "the gotcha store {} cannot be read ({e}); it was left as it is and nothing was \
         saved: repair the file or move it away",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_errors_persist_across_load_for_cross_process_correlation() {
        // In `sobre -c` mode every command is its own process, so the fix
        // correlation only works if pending errors are persisted. Simulate
        // process 1 (failing build) → process 2 (green run).
        let _iso = crate::core::data_dir::isolated_data_dir();
        let root = "/tmp/sobre-gotcha-persist-test-xyzzy";

        let mut p1 = GotchaStore::load(root);
        assert!(p1.learn_from_shell(
            "cargo build",
            "error[E0382]: borrow of moved value: `x`",
            1,
            &[],
            "p1",
        ));
        assert_eq!(p1.pending_errors.len(), 1);
        p1.save(root).unwrap();

        // A fresh process must see the persisted pending error.
        let mut p2 = GotchaStore::load(root);
        assert_eq!(
            p2.pending_errors.len(),
            1,
            "pending error must survive a reload for cross-process correlation"
        );
        let changed = p2.learn_from_shell(
            "cargo build",
            "Finished `dev` profile [unoptimized + debuginfo]",
            0,
            &["src/main.rs".into()],
            "p2",
        );
        assert!(
            changed,
            "green run must correlate the persisted pending error"
        );
        assert_eq!(p2.gotchas.len(), 1, "fix correlates across processes");
        assert_eq!(p2.pending_errors.len(), 0, "pending consumed by the fix");
    }

    #[test]
    fn an_unreadable_store_is_never_overwritten() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let root = "/tmp/sobre-gotcha-corrupt-store";
        let path = gotcha_path(&crate::core::project_hash::hash_project_root(root)).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let garbage = "{\"gotchas\": [ truncated";
        std::fs::write(&path, garbage).unwrap();

        let mut ran = false;
        let err = GotchaStore::update(root, |_| {
            ran = true;
            true
        })
        .expect_err("a store that does not parse must not be written");
        assert!(!ran, "the update must not run on a store it could not read");
        assert!(err.contains(&path.display().to_string()), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), garbage);
        assert!(GotchaStore::load(root).gotchas.is_empty());
    }
}
