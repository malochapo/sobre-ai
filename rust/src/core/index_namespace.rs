use std::path::{Path, PathBuf};

use md5::{Digest, Md5};

pub(crate) fn vectors_dir(project_root: &Path) -> PathBuf {
    let hash = namespace_hash(project_root);
    if let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() {
        return data_dir.join("vectors").join(hash);
    }

    PathBuf::from(".").join("vectors").join(hash)
}

pub(crate) fn namespace_hash(project_root: &Path) -> String {
    let seed = namespace_seed(project_root);
    let mut hasher = Md5::new();
    hasher.update(seed.as_bytes());
    crate::core::hasher::hex_encode(&hasher.finalize())
}

fn namespace_seed(project_root: &Path) -> String {
    let root_s = project_root.to_string_lossy().to_string();
    crate::core::project_hash::project_identity(&root_s)
        .unwrap_or_else(|| crate::core::index_paths::normalize_project_root(&root_s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_hash_is_stable_across_clones_with_same_git_remote() {
        let _env = crate::core::data_dir::test_env_lock();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(a.path().join(".git")).unwrap();
        std::fs::write(
            a.path().join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:user/my-repo.git\n",
        )
        .unwrap();

        std::fs::create_dir_all(b.path().join(".git")).unwrap();
        std::fs::write(
            b.path().join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:user/my-repo.git\n",
        )
        .unwrap();

        let ha = namespace_hash(a.path());
        let hb = namespace_hash(b.path());
        assert_eq!(ha, hb);
    }
}
