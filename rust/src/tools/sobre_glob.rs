use std::path::Path;

use ignore::WalkBuilder;

use crate::core::protocol;
use crate::core::tokens::count_tokens;

/// Hard ceiling on the number of files returned from a single glob search,
/// independent of the caller-supplied `max_results`.
const MAX_RESULTS: usize = 500;

/// Finds files matching a glob `pattern` under `dir` with compressed output.
///
/// Unlike `sobre_search` which matches file *content*, this matches file *paths*.
/// Uses the `ignore` crate for gitignore-aware, hidden-aware walking and matches
/// against the standard `glob` crate's pattern syntax (`*.rs`, `**/*.ts`, …).
///
/// The walk is ordered by path (`sort_by_file_path`) so that — even when the
/// result set is truncated to `max_results` — the *set* of returned files, not
/// just their printed order, is deterministic across runs.
///
/// Returns `(output, original_tokens)`. On error the output starts with
/// `"ERROR:"` and `original_tokens` is `0`.
pub fn handle(
    pattern: &str,
    dir: &str,
    respect_gitignore: bool,
    max_results: usize,
) -> (String, usize) {
    let requested_root = Path::new(dir);
    let walk_root = crate::core::walk_filter::explicit_walk_root(requested_root);
    let root = walk_root.as_path();
    if !root.exists() {
        return (format!("ERROR: {dir} does not exist"), 0);
    }
    if !root.is_dir() {
        return (format!("ERROR: {dir} is not a directory"), 0);
    }

    let max = max_results.min(MAX_RESULTS);

    // Support both simple (`*.rs`) and recursive (`**/*.ts`) patterns.
    let glob_matcher = match glob::Pattern::new(pattern) {
        Ok(m) => m,
        Err(e) => return (format!("ERROR: invalid glob pattern '{pattern}': {e}"), 0),
    };

    let mut matches = Vec::new();
    let mut files_walked = 0u32;
    let mut stopped_at_cap = false;

    // Vendor dirs (node_modules, …) follow the gitignore toggle: explicitly
    // disabling gitignore is the escape hatch to look inside them.
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .require_git(false)
        .filter_entry(move |e| {
            if respect_gitignore {
                crate::core::walk_filter::keep_entry(e)
            } else {
                crate::core::cloud_files::keep_entry(e)
            }
        })
        .sort_by_file_path(std::path::Path::cmp)
        .build();

    for entry in walker.filter_map(std::result::Result::ok) {
        if matches.len() >= max {
            stopped_at_cap = true;
            break;
        }

        // Skip directories; only files are matchable results.
        if entry.file_type().is_none_or(|ft| ft.is_dir()) {
            continue;
        }
        // Symlinks are not followed (the `rg --files` default).
        if entry.file_type().is_some_and(|ft| ft.is_symlink()) {
            continue;
        }

        let path = entry.path();
        files_walked += 1;

        let rel_path = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel_path.to_string_lossy();

        if glob_matcher.matches(&rel_str) {
            let short_path =
                protocol::shorten_path_relative(&path.to_string_lossy(), &root.to_string_lossy());
            matches.push(short_path);
        }
    }

    let not_walked = crate::tools::sobre_search::not_searched_note(respect_gitignore)
        .replace("not searched", "not walked");
    if matches.is_empty() {
        return (
            format!("0 files matched '{pattern}' in {files_walked} files walked ({not_walked})"),
            0,
        );
    }

    // Deterministic output ordering (the walk is already path-ordered; this also
    // normalises the shortened-path representation).
    matches.sort();

    let output = matches.join("\n");
    let raw_tokens = count_tokens(&output);

    let mut footer = format!(
        "\n\n{} files matched (walked {files_walked})",
        matches.len()
    );
    if stopped_at_cap {
        footer.push_str(&format!(
            "\n(stopped at max_results={max}: more files may match — raise max_results \
             or narrow the pattern)"
        ));
    }
    footer.push_str(&format!("\n({not_walked})"));
    let full_output = format!("{output}{footer}");

    // A plain file list carries no compression overhead, so the original token
    // budget equals what we send.
    (full_output, raw_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// G12: a glob that stops at `max_results` says so.
    #[test]
    fn stopping_at_max_results_is_said() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..6 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let (capped, _) = handle("*.txt", &dir.path().to_string_lossy(), true, 3);
        assert!(capped.contains("stopped at max_results=3"), "{capped}");
        assert!(capped.contains("not walked: hidden files"), "{capped}");
        let (all, _) = handle("*.txt", &dir.path().to_string_lossy(), true, 100);
        assert!(all.contains("6 files matched"), "{all}");
        assert!(!all.contains("stopped at max_results"), "{all}");
    }

    #[test]
    fn glob_results_are_deterministically_ordered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "content").unwrap();
        std::fs::write(dir.path().join("a.txt"), "content").unwrap();
        std::fs::write(dir.path().join("c.rs"), "content").unwrap();

        let (out, _) = handle("*.txt", &dir.path().to_string_lossy(), true, 100);

        let lines: Vec<&str> = out
            .lines()
            .filter(|l| {
                std::path::Path::new(l)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
            })
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0] < lines[1], "results must be sorted: {lines:?}");
    }

    #[test]
    fn glob_skips_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let (out, _) = handle("**/*.txt", &dir.path().to_string_lossy(), true, 100);

        assert!(out.contains("file.txt"));
        assert!(!out.contains("subdir"));
    }

    #[test]
    fn glob_recursive_pattern_descends_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested").join("deep.rs"), "fn x() {}").unwrap();
        std::fs::write(dir.path().join("top.rs"), "fn y() {}").unwrap();

        let (out, _) = handle("**/*.rs", &dir.path().to_string_lossy(), true, 100);

        assert!(
            out.contains("deep.rs"),
            "recursive glob must descend: {out}"
        );
        assert!(out.contains("top.rs"));
    }

    #[test]
    fn glob_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        // The `ignore` crate only honours .gitignore inside a git repo (its
        // `require_git` default); mark the tempdir as a repo root so the test
        // exercises real-world behaviour without shelling out to `git`.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.rs\n").unwrap();
        std::fs::write(dir.path().join("ignored.rs"), "fn a() {}").unwrap();
        std::fs::write(dir.path().join("kept.rs"), "fn b() {}").unwrap();

        let (respected, _) = handle("**/*.rs", &dir.path().to_string_lossy(), true, 100);
        assert!(respected.contains("kept.rs"));
        assert!(
            !respected.contains("ignored.rs"),
            "gitignored file must be skipped: {respected}"
        );

        // With gitignore disabled, the ignored file reappears.
        let (unrespected, _) = handle("**/*.rs", &dir.path().to_string_lossy(), false, 100);
        assert!(unrespected.contains("ignored.rs"));
    }

    #[test]
    fn glob_invalid_pattern_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let (out, _) = handle("[invalid", &dir.path().to_string_lossy(), true, 100);

        assert!(out.starts_with("ERROR:"));
        assert!(out.contains("invalid glob pattern"));
    }

    #[test]
    fn glob_nonexistent_dir_returns_error() {
        let (out, _) = handle("*.txt", "/nonexistent/path", true, 100);

        assert!(out.starts_with("ERROR:"));
        assert!(out.contains("does not exist"));
    }

    #[test]
    fn glob_respects_max_results() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("file{i}.txt")), "content").unwrap();
        }

        let (out, _) = handle("*.txt", &dir.path().to_string_lossy(), true, 5);

        let file_lines: Vec<&str> = out
            .lines()
            .filter(|l| {
                std::path::Path::new(l)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
            })
            .collect();
        assert!(file_lines.len() <= 5, "should respect max_results");
    }

    #[cfg(windows)]
    #[test]
    fn glob_walks_explicit_directory_reparse_root() {
        use std::os::windows::fs::symlink_dir;

        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("target");
        let link = tmp.path().join("junction");
        std::fs::create_dir_all(&target).expect("target");
        std::fs::write(target.join("visible.rs"), "fn visible() {}\n").expect("fixture");
        if symlink_dir(&target, &link).is_err() {
            return;
        }
        let (out, _) = handle("**/*.rs", &link.to_string_lossy(), true, 20);
        assert!(
            out.contains("visible.rs"),
            "junction root must be traversed: {out}"
        );
    }
}
