//! Index path utilities — project-root normalization and graph match keys.
//!
//! Pure path helpers shared by the graph index, the property graph and the
//! graph tools. They have no dependency on any graph backend — only `pathutil`
//! and std.

use std::path::Path;

pub(crate) fn normalize_absolute_path(path: &str) -> String {
    if let Ok(canon) = crate::core::pathutil::safe_canonicalize(std::path::Path::new(path)) {
        return canon.to_string_lossy().to_string();
    }

    let mut normalized = path.to_string();
    while normalized.ends_with("\\.") || normalized.ends_with("/.") {
        normalized.truncate(normalized.len() - 2);
    }
    while normalized.len() > 1
        && (normalized.ends_with('\\') || normalized.ends_with('/'))
        && !normalized.ends_with(":\\")
        && !normalized.ends_with(":/")
        && normalized != "\\"
        && normalized != "/"
    {
        normalized.pop();
    }
    normalized
}

pub fn normalize_project_root(path: &str) -> String {
    normalize_absolute_path(path)
}

pub fn graph_match_key(path: &str) -> String {
    let stripped =
        crate::core::pathutil::strip_verbatim_str(path).unwrap_or_else(|| path.replace('\\', "/"));
    stripped.trim_start_matches('/').to_string()
}

pub fn graph_relative_key(path: &str, root: &str) -> String {
    let root_norm = normalize_project_root(root);
    let path_norm = normalize_absolute_path(path);
    let root_path = Path::new(&root_norm);
    let path_path = Path::new(&path_norm);

    if let Ok(rel) = path_path.strip_prefix(root_path) {
        return slash_key(&rel.to_string_lossy());
    }
    slash_key(path)
}

/// The graph keys files with `/` on every platform: a canonical path comes
/// back with `/` on Windows too (`pathutil::strip_verbatim_str`), and the
/// index stores the part below the root of that path.
fn slash_key(path: &str) -> String {
    let path = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    path.trim_start_matches(['/', '\\']).to_string()
}
