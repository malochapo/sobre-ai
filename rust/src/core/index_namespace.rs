use std::path::{Path, PathBuf};

/// The project's index directory, or `None` when there is no data dir (then
/// nothing is read or written).
pub(crate) fn vectors_dir(project_root: &Path) -> Option<PathBuf> {
    let data_dir = crate::core::data_dir::sobre_data_dir().ok()?;
    Some(data_dir.join("vectors").join(namespace_hash(project_root)))
}

/// The project's key, as every store has it: see
/// [`crate::core::project_hash::hash_project_root`].
pub(crate) fn namespace_hash(project_root: &Path) -> String {
    crate::core::project_hash::hash_project_root(&project_root.to_string_lossy())
}
