use std::path::Path;

use ignore::WalkBuilder;

use crate::core::tokens::count_tokens;

/// A compact directory tree with each directory's file count, and the
/// tokens the same entries cost as a plain path list (the `original` the
/// usage ledger compares against). Hidden and ignored entries, and what lies
/// past `depth`, are not listed, and the answer says so.
pub fn handle(
    path: &str,
    depth: usize,
    show_hidden: bool,
    respect_gitignore: bool,
) -> (String, usize) {
    let requested_root = Path::new(path);
    let walk_root = crate::core::walk_filter::explicit_walk_root(requested_root);
    let root = walk_root.as_path();
    if root.is_file() {
        let parent = root
            .parent()
            .map_or(path.to_string(), |p| p.display().to_string());
        return (
            format!(
                "ERROR: '{path}' is a file, not a directory. Use path=\"{parent}\" for the containing directory."
            ),
            0,
        );
    }
    if !root.is_dir() {
        return (
            format!("ERROR: {path} does not exist or is not a directory"),
            0,
        );
    }

    let entries = walk(root, depth, show_hidden, respect_gitignore);
    if entries.is_empty() {
        let present = std::fs::read_dir(root).map_or(0, Iterator::count);
        let why = if present == 0 {
            "empty directory".to_string()
        } else if depth == 0 {
            format!("{present} entries, none listed at depth=0")
        } else {
            format!("{present} entries, all hidden or ignored")
        };
        return (format!("{path}/ ({why})"), 0);
    }

    let listed = entries.len();
    let plain: Vec<String> = entries
        .iter()
        .map(|e| {
            e.path
                .strip_prefix(root)
                .unwrap_or(&e.path)
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let mut out = render(&entries, depth);
    let all = walk(root, depth, true, false).len();
    if all > listed {
        out.push_str(&format!(
            "\n[{} hidden or ignored entries not listed]",
            all - listed
        ));
    }
    if entries.iter().any(|e| e.is_dir && e.depth == depth) {
        out.push_str(&format!(
            "\n[directories marked … go deeper than depth={depth}]"
        ));
    }
    (out, count_tokens(&plain.join("\n")))
}

struct Entry {
    depth: usize,
    name: String,
    is_dir: bool,
    path: std::path::PathBuf,
}

/// The entries under `root` to `max_depth`, sorted. Vendor dirs
/// (node_modules, …) follow the gitignore toggle: explicitly disabling
/// gitignore is the escape hatch to look inside them.
fn walk(root: &Path, max_depth: usize, show_hidden: bool, respect_gitignore: bool) -> Vec<Entry> {
    WalkBuilder::new(root)
        .hidden(!show_hidden)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .require_git(false)
        .max_depth(Some(max_depth))
        .sort_by_file_name(std::cmp::Ord::cmp)
        .filter_entry(move |e| {
            if respect_gitignore {
                crate::core::walk_filter::keep_entry(e)
            } else {
                crate::core::cloud_files::keep_entry(e)
            }
        })
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.depth() > 0)
        .map(|e| Entry {
            depth: e.depth(),
            name: e.file_name().to_string_lossy().to_string(),
            is_dir: e.file_type().is_some_and(|ft| ft.is_dir()),
            path: e.path().to_path_buf(),
        })
        .collect()
}

/// One line per entry, indented; a directory carries the number of files
/// listed in it, or `…` when it sits at the depth limit and its contents
/// were not walked.
fn render(entries: &[Entry], max_depth: usize) -> String {
    let mut dir_file_counts: std::collections::HashMap<&Path, usize> =
        std::collections::HashMap::new();
    for e in entries {
        if !e.is_dir
            && let Some(parent) = e.path.parent()
        {
            *dir_file_counts.entry(parent).or_default() += 1;
        }
    }
    entries
        .iter()
        .map(|e| {
            let indent = "  ".repeat(e.depth.saturating_sub(1));
            if !e.is_dir {
                format!("{indent}{}", e.name)
            } else if e.depth == max_depth {
                format!("{indent}{}/ …", e.name)
            } else {
                let count = dir_file_counts.get(e.path.as_path()).copied().unwrap_or(0);
                format!("{indent}{}/ ({count})", e.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a deterministic source-tree fixture so the assertions do not
    /// depend on the live repository size or platform path separators (the live
    /// repo coupling previously made this test tip over its token threshold on
    /// Windows as the codebase grew).
    fn make_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let files = [
            "Cargo.toml",
            "README.md",
            "src/main.rs",
            "src/lib.rs",
            "src/core/mod.rs",
            "src/core/engine.rs",
            "src/core/util.rs",
            "src/tools/mod.rs",
            "src/tools/reader.rs",
            "tests/integration.rs",
            "tests/smoke.rs",
        ];
        for rel in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "// fixture\n").unwrap();
        }
        dir
    }

    #[test]
    fn tree_savings_are_reasonable() {
        let dir = make_fixture();
        let (output, original) = handle(&dir.path().to_string_lossy(), 3, false, true);
        let compact_tokens = count_tokens(&output);

        eprintln!("=== sobre_tree savings test ===");
        eprintln!("  original (raw) tokens: {original}");
        eprintln!("  compact tokens:        {compact_tokens}");
        eprintln!(
            "  savings:               {}",
            original.saturating_sub(compact_tokens)
        );

        assert!(original > 0, "raw tree should have some tokens");
        assert!(
            original < 2000,
            "raw tree for the fixture should be small, got {original}"
        );
        if original > compact_tokens {
            let ratio = (original - compact_tokens) as f64 / original as f64;
            eprintln!("  savings ratio:         {:.1}%", ratio * 100.0);
            assert!(
                ratio < 0.90,
                "savings ratio should be < 90% for same-depth comparison, got {:.1}%",
                ratio * 100.0
            );
        }
    }

    /// A directory at the depth limit says its contents were not
    /// walked; a directory of only dotfiles is not "empty"; what was left
    /// out is said.
    #[test]
    fn tree_says_what_it_left_out() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::write(root.join("a/b/c/deep.rs"), "x").unwrap();
        std::fs::write(root.join(".hidden"), "x").unwrap();
        let (out, _) = handle(&root.to_string_lossy(), 2, false, true);
        assert!(out.contains("b/ …"), "{out}");
        assert!(!out.contains("b/ (0)"), "{out}");
        assert!(
            out.contains("1 hidden or ignored entries not listed"),
            "{out}"
        );

        let only_env = tempfile::tempdir().expect("tempdir");
        std::fs::write(only_env.path().join(".env"), "X=1").unwrap();
        let (out, _) = handle(&only_env.path().to_string_lossy(), 3, false, true);
        assert!(out.contains("all hidden or ignored"), "{out}");
        let (out, _) = handle(&root.to_string_lossy(), 0, false, true);
        assert!(out.contains("none listed at depth=0"), "{out}");
    }

    /// The original is the same entries as a plain list, not a walk
    /// that also counts what was filtered out.
    #[test]
    fn the_original_counts_only_the_listed_entries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("node_modules/pkg")).unwrap();
        for i in 0..200 {
            std::fs::write(tmp.path().join(format!("node_modules/pkg/f{i}.js")), "x").unwrap();
        }
        std::fs::write(tmp.path().join("main.rs"), "x").unwrap();
        let (_, original) = handle(&tmp.path().to_string_lossy(), 4, false, true);
        assert!(
            original < 20,
            "node_modules must not count as saved: {original}"
        );
    }

    #[test]
    fn tree_hides_node_modules_by_default_even_without_git() {
        // Vendor dirs are pruned by default; respect_gitignore=false is
        // the explicit escape hatch to look inside them.
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("node_modules/react")).expect("mkdir");
        std::fs::write(tmp.path().join("node_modules/react/index.js"), "x").expect("write");
        std::fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
        std::fs::write(tmp.path().join("src/app.js"), "y").expect("write");
        let root = tmp.path().to_string_lossy().to_string();

        let (default_out, _) = handle(&root, 4, false, true);
        assert!(default_out.contains("src"), "src visible: {default_out}");
        assert!(
            !default_out.contains("node_modules"),
            "node_modules must be hidden by default: {default_out}"
        );

        let (opt_out, _) = handle(&root, 4, false, false);
        assert!(
            opt_out.contains("node_modules"),
            "respect_gitignore=false must reveal vendor dirs: {opt_out}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn tree_walks_explicit_directory_reparse_root() {
        use std::os::windows::fs::symlink_dir;

        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("target");
        let link = tmp.path().join("junction");
        std::fs::create_dir_all(&target).expect("target");
        std::fs::write(target.join("visible.rs"), "fn visible() {}\n").expect("fixture");
        if symlink_dir(&target, &link).is_err() {
            // Windows hosts without Developer Mode cannot create this fixture.
            return;
        }
        let (out, _) = handle(&link.to_string_lossy(), 2, false, true);
        assert!(
            out.contains("visible.rs"),
            "junction root must be traversed: {out}"
        );
    }
}
