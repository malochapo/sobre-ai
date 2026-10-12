use std::path::Path;

/// The key of every per-project store (graphs, search indexes,
/// co-occurrence, checkpoints): a short hash of the project's
/// canonical root. Two clones are two projects, and adding or changing a git
/// remote or a package name moves nothing.
///
/// The root is canonicalized (symlinks resolved), then normalized so the same
/// directory always gives the same key whichever interface named it: on
/// Windows the MCP server reports `D:/repo` and the CLI `D:\repo`.
pub(crate) fn hash_project_root(root: &str) -> String {
    let canonical = crate::core::pathutil::safe_canonicalize_or_self(Path::new(root));
    let normalized = crate::core::pathutil::normalize_tool_path(&canonical.to_string_lossy());
    crate::core::hasher::hash_short(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_slash_and_backslash_hash_identically() {
        assert_eq!(
            hash_project_root(r"D:\repos\oref-examples"),
            hash_project_root("D:/repos/oref-examples"),
        );
    }

    #[test]
    fn trailing_slash_does_not_split_hash() {
        assert_eq!(
            hash_project_root("/home/user/project/"),
            hash_project_root("/home/user/project"),
        );
    }

    /// Two clones of one repository are two projects.
    #[test]
    fn two_clones_with_one_remote_are_two_projects() {
        let config = "[remote \"origin\"]\n\turl = git@github.com:user/repo.git\n";
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir.path().join(".git")).unwrap();
            std::fs::write(dir.path().join(".git").join("config"), config).unwrap();
        }
        assert_ne!(
            hash_project_root(&a.path().to_string_lossy()),
            hash_project_root(&b.path().to_string_lossy()),
        );
    }

    /// Adding a remote or renaming the package leaves the key alone.
    #[test]
    fn a_new_remote_or_package_name_moves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().to_string();
        let before = hash_project_root(&root);
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git").join("config"),
            "[remote \"origin\"]\n\turl = https://example.com/x.git\n",
        )
        .unwrap();
        assert_eq!(hash_project_root(&root), before);
    }

    /// A root named through a symlink is the same project.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_is_the_same_project() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            hash_project_root(&real.to_string_lossy()),
            hash_project_root(&link.to_string_lossy()),
        );
    }
}
