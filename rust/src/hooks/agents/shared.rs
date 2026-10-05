use std::path::PathBuf;

use super::super::{
    REDIRECT_SCRIPT_GENERIC, generate_compact_rewrite_script, is_inside_git_repo, make_executable,
    resolve_binary_path_for_bash, write_file, write_wrapper_file,
};

pub(super) fn install_standard_hook_scripts(
    hooks_dir: &std::path::Path,
    home: &std::path::Path,
    rewrite_name: &str,
    redirect_name: &str,
) {
    let _ = std::fs::create_dir_all(hooks_dir);

    // #719: never re-stamp a working portable wrapper with an absolute path.
    let binary = resolve_binary_path_for_bash();
    let rewrite_path = hooks_dir.join(rewrite_name);
    let rewrite_script = generate_compact_rewrite_script(&binary);
    write_wrapper_file(&rewrite_path, &rewrite_script, home);
    make_executable(&rewrite_path);

    let redirect_path = hooks_dir.join(redirect_name);
    write_file(&redirect_path, REDIRECT_SCRIPT_GENERIC);
    make_executable(&redirect_path);
}

pub(super) fn prepare_project_rules_path(global: bool, file_name: &str) -> Option<PathBuf> {
    if global {
        eprintln!(
            "Global mode: skipping project-local {file_name} (use without --global in a project)."
        );
        return None;
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    if !is_inside_git_repo(&cwd) || cwd == crate::core::home::resolve_home_dir().unwrap_or_default()
    {
        eprintln!("  Skipping {file_name}: not inside a git repository or in home directory.");
        return None;
    }

    let rules_path = PathBuf::from(file_name);
    if rules_path.exists() {
        let content = std::fs::read_to_string(&rules_path).unwrap_or_default();
        if content.contains("lean-ctx") {
            eprintln!("{file_name} already configured.");
            return None;
        }
    }

    Some(rules_path)
}

/// Remove the first lean-ctx block delimited by `start`..`end` from `content`.
/// Shared by the Claude/CodeBuddy CLAUDE.md/CODEBUDDY.md installers and `doctor`.
/// Markers match as whole (trimmed) lines only (GL #1158) — a prose mention of
/// a marker must never trigger block surgery.
pub(super) fn remove_block(content: &str, start: &str, end: &str) -> String {
    let s = crate::marked_block::marker_line_span(content, start);
    let e = s.and_then(|(si, _)| {
        crate::marked_block::marker_line_span(&content[si..], end)
            .map(|(es, ee)| (si + es, si + ee))
    });
    match (s, e) {
        (Some((si, _)), Some((_, end_after))) => {
            let before = content[..si].trim_end_matches('\n');
            let after = &content[end_after..];
            let mut out = before.to_string();
            out.push('\n');
            if !after.trim().is_empty() {
                out.push('\n');
                out.push_str(after.trim_start_matches('\n'));
            }
            out
        }
        _ => content.to_string(),
    }
}

/// Remove *every* lean-ctx block delimited by `start`..`end`. Heals files that
/// accumulated duplicate blocks from the pre-#549 marker mismatch (the detector
/// constant pointed at `<!-- lean-ctx-rules -->` while the written block used
/// `<!-- lean-ctx -->`, so every `setup`/`doctor --fix` appended a fresh copy).
/// Callers then write exactly one canonical block back.
pub(super) fn remove_all_blocks(content: &str, start: &str, end: &str) -> String {
    let mut out = content.to_string();
    while crate::marked_block::contains_marker_line(&out, start) {
        let next = remove_block(&out, start, end);
        if next == out {
            break; // malformed (start without end) — avoid an infinite loop
        }
        out = next;
    }
    out
}
