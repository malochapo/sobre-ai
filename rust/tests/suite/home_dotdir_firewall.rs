//! Home dot-dir firewall — regression guard for the GH #434–#436 class.
//!
//! Building a HOME-based `~/.sobre` dir directly (e.g.
//! `home.join(".sobre")`) bypasses the XDG resolver in `core::paths` /
//! `core::data_dir`, so files would land outside the typed `$XDG_*` dirs.
//! Code MUST go through `data_dir()` / `state_dir()`
//! / `config_dir()` / `cache_dir()` instead.
//!
//! No file constructs a home-based `~/.sobre` path; this test fails if one
//! appears.
//!
//! Project-local `<project>/.sobre` directories (the per-repo index, sibling
//! to `.git`) are a different, legitimate concept and are intentionally NOT
//! flagged — only joins rooted at the user HOME are matched.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// True when the file builds the HOME dot-dir `~/.sobre` directly: some
/// `join(".sobre…")` is rooted at a `home` token (`home.join(".sobre")`,
/// `dirs::home_dir()…join(".sobre/logs")`, `…join(".sobre/agents/…")`).
///
/// Both the bare dir (`join(".sobre")`) and any subpath join
/// (`join(".sobre/logs")`, `join(".sobre/agents/tasks.json")`) are
/// matched: the byte right after `.sobre` must be `"` or `/`, so other names
/// that merely share the prefix (followed by `.` / `-`) stay out of scope.
///
/// The scan spans lines on purpose — the join is frequently chained on its own
/// line below `dirs::home_dir()`, which a line-by-line check would miss. A
/// bounded, char-boundary safe look-back keeps project-local roots
/// (`project_root`, `root`, `cwd`, `dir`) — which carry no `home` token — out
/// of scope by design.
fn builds_home_dotdir_path(text: &str) -> bool {
    const PREFIX: &str = r#"join(".sobre"#;
    const LOOKBACK: usize = 160;
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(PREFIX) {
        let idx = search_from + rel;
        let after = idx + PREFIX.len();
        // The dir itself (`"`) or a subpath under it (`/`).
        if matches!(bytes.get(after), Some(b'"' | b'/')) {
            let mut start = idx.saturating_sub(LOOKBACK);
            while start > 0 && !text.is_char_boundary(start) {
                start -= 1;
            }
            if text[start..idx].contains("home") {
                return true;
            }
        }
        search_from = after;
    }
    false
}

#[test]
fn no_home_dotdir_path_construction() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    assert!(!files.is_empty(), "no source files found under {src:?}");

    let mut offenders = BTreeSet::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        if builds_home_dotdir_path(&text) {
            let rel = file
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            offenders.insert(rel);
        }
    }

    assert!(
        offenders.is_empty(),
        "Home-based `~/.sobre` path construction detected.\n\
         Use core::paths (data_dir/state_dir/config_dir/cache_dir) instead.\n\
         Offending file(s):\n  {}",
        offenders
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
