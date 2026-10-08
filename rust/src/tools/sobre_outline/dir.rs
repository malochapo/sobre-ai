//! Directory ("folder surface") outline: walk a directory and emit a
//! deterministic per-file table of contents. Mirrors `ast-grep outline <dir>` —
//! gitignore/vendor aware, bounded, sorted.

use crate::core::signatures::extract_signatures_with_backend;
use crate::core::tokens::count_tokens;
use crate::tools::CrpMode;

use super::{FileSymbols, OutlineOpts, filter_signatures, render_one};

/// Hard ceiling on files included, so a huge tree can't blow up the output.
const MAX_FILES: usize = 600;
/// Skip individual files larger than this — outlining a multi-MB generated file
/// is rarely the intent and parsing it is needlessly expensive.
const MAX_FILE_BYTES: u64 = 1_500_000;

pub(super) fn outline_dir(dir: &str, opts: &OutlineOpts) -> (String, usize) {
    let (files, total_tokens, truncated) = collect(dir, opts);

    if opts.as_json {
        return (super::json::dir_json(dir, &files, truncated), total_tokens);
    }

    if files.is_empty() {
        return (super::no_match_message(dir, opts), 0);
    }

    let crp = CrpMode::effective();
    let mut out = String::new();
    for f in &files {
        out.push_str(&f.rel);
        out.push('\n');
        for s in &f.sigs {
            out.push_str("  ");
            out.push_str(&render_one(s, crp));
            out.push('\n');
        }
    }
    if truncated {
        out.push_str(&format!(
            "[truncated at {MAX_FILES} files — narrow the path for the rest]\n"
        ));
    }
    // Located symbols are addressable as stable handles (#607): one self-
    // describing hint for the whole listing (path is each file's group header).
    out.push_str(crate::core::handle::USAGE_HINT);
    out.push('\n');

    (out.clone(), total_tokens)
}

/// Walk `dir` (gitignore/vendor aware), extract + filter per file, and return
/// the files with ≥1 surviving symbol — sorted by relative path for
/// determinism — plus the full-read token baseline and whether the cap hit.
fn collect(dir: &str, opts: &OutlineOpts) -> (Vec<FileSymbols>, usize, bool) {
    let mut paths: Vec<std::path::PathBuf> = ignore::WalkBuilder::new(dir)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(crate::core::walk_filter::keep_entry)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        .map(|e| e.path().to_path_buf())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(crate::core::language_capabilities::is_indexable_ext)
        })
        .collect();
    paths.sort();
    let truncated = paths.len() > MAX_FILES;
    paths.truncate(MAX_FILES);

    let root = std::path::Path::new(dir);
    let mut files = Vec::new();
    let mut total_tokens = 0usize;

    for path in paths {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let (sigs, backend) = extract_signatures_with_backend(&content, &ext);
        let filtered: Vec<_> = filter_signatures(&sigs, opts)
            .into_iter()
            .cloned()
            .collect();
        if filtered.is_empty() {
            continue;
        }
        total_tokens += count_tokens(&content);
        files.push(FileSymbols {
            rel: rel_path(root, &path),
            ext,
            backend,
            sigs: filtered,
        });
    }
    (files, total_tokens, truncated)
}

/// Path relative to `root`, normalized to forward slashes for deterministic,
/// OS-independent output. Falls back to the absolute path if stripping fails.
fn rel_path(root: &std::path::Path, path: &std::path::Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let s = rel.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        s.into_owned()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}
