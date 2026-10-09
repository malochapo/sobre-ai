//! Search-root resolution and result scoping (the requested subdirectory).

use std::path::{Path, PathBuf};

/// Resolve the index root for a search/index path.
///
/// The BM25 namespace is keyed on the detected *project* root, not on the
/// literal search path: a search launched from — or pointed at — a
/// subdirectory would otherwise hash to a different, usually empty namespace
/// and return zero hits, even though the real index sits one directory up. Promoting the search path the same way the
/// build does makes both agree. A genuinely requested subdirectory is kept as a
/// result-scope filter (second tuple field) rather than becoming its own
/// namespace.
pub(crate) fn resolve_search_root(path: &str) -> Result<(PathBuf, Option<String>), String> {
    let raw = Path::new(path);
    if !raw.exists() {
        return Err(format!("path does not exist: {path}"));
    }
    let raw_dir = if raw.is_file() {
        raw.parent().unwrap_or(raw)
    } else {
        raw
    };
    let root = PathBuf::from(crate::core::protocol::detect_project_root_or_cwd(
        &raw_dir.to_string_lossy(),
    ));
    let subdir = search_subdir_filter(&root, raw_dir);
    Ok((root, subdir))
}

/// Project-relative prefix (forward slashes, no leading/trailing slash) for
/// `requested` under `root`, or `None` when `requested` is the root itself or
/// not contained in it. Lets a subdirectory search stay scoped after the path
/// was promoted to the project root for the index namespace.
pub(crate) fn search_subdir_filter(root: &Path, requested: &Path) -> Option<String> {
    let root_c = crate::core::pathutil::safe_canonicalize_or_self(root);
    let req_c = crate::core::pathutil::safe_canonicalize_or_self(requested);
    let rel = req_c.strip_prefix(&root_c).ok()?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    let rel = rel.trim_matches('/').to_string();
    if rel.is_empty() { None } else { Some(rel) }
}

/// Keeps a search's results to the subdirectory the caller asked for, when
/// the search was promoted to the project root for the index namespace.
#[derive(Default)]
pub(crate) struct SearchFilter {
    /// Relative directory prefix (forward slashes, no trailing slash).
    subdir: Option<String>,
}

impl SearchFilter {
    /// Scope results to a project-relative subdirectory. `None` (or empty)
    /// clears the scope.
    pub(crate) fn with_subdir(mut self, subdir: Option<String>) -> Self {
        self.subdir = subdir.filter(|s| !s.is_empty());
        self
    }

    pub(crate) fn is_active(&self) -> bool {
        self.subdir.is_some()
    }

    pub(crate) fn matches(&self, rel_path: &str) -> bool {
        let rel_path = rel_path.replace('\\', "/");
        match &self.subdir {
            Some(prefix) => rel_path == *prefix || rel_path.starts_with(&format!("{prefix}/")),
            None => true,
        }
    }
}
