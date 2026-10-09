use super::model::{GotchaStore, MAX_PENDING};
use std::path::PathBuf;

impl GotchaStore {
    pub fn load(project_root: &str) -> Self {
        let hash = crate::core::project_hash::hash_project_root(project_root);
        if let Some(path) = gotcha_path(&hash)
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(mut store) = serde_json::from_str::<GotchaStore>(&content)
        {
            store.apply_decay();
            store.cross_session_boost();
            // Shell-ding: keep persisted pending errors so a fail→fix that
            // spans two `sobre -c` processes still correlates — but drop
            // expired ones (15-min TTL) and bound to the most recent MAX_PENDING
            // so a stale file can never grow unbounded or resurrect old errors.
            store.pending_errors.retain(|p| !p.is_expired());
            if store.pending_errors.len() > MAX_PENDING {
                let excess = store.pending_errors.len() - MAX_PENDING;
                store.pending_errors.drain(0..excess);
            }
            return store;
        }
        Self::new(&hash)
    }

    pub fn save(&self, project_root: &str) -> Result<(), String> {
        let hash = crate::core::project_hash::hash_project_root(project_root);
        let path = gotcha_path(&hash).ok_or("no data dir")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }
}

fn gotcha_path(project_hash: &str) -> Option<PathBuf> {
    Some(
        crate::core::data_dir::sobre_data_dir()
            .ok()?
            .join("knowledge")
            .join(project_hash)
            .join("gotchas.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_errors_persist_across_load_for_cross_process_correlation() {
        // Shell-ding: in `sobre -c` mode every command is its own
        // process, so the fix correlation can only work if pending errors are
        // persisted. Simulate process 1 (failing build) → process 2 (green run).
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

        // Fresh process must SEE the persisted pending error (was cleared before).
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
}
