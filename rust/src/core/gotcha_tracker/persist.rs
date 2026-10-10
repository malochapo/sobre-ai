use super::model::GotchaStore;
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
