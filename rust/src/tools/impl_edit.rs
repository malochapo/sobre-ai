//! `sobre_patch`'s text replacement: `replace_unique` (one exact match of
//! `old_text`) and `replace_all` (every match of `find`).

use std::path::Path;

use crate::core::cache::SessionCache;
use crate::core::tokens::count_tokens;
// Shared TOCTOU-safe read→verify→atomic-write primitives: the same
// audited boundary the anchored ops build on, so a fix protects both.
use crate::tools::edit_io::{
    FilePreimage, ensure_preimage_still_matches, read_preimage, write_atomic_bytes_with_permissions,
};

/// Lines of diff an edit result shows.
pub(crate) const DIFF_MAX_LINES: usize = 200;

/// A text replacement in one file.
pub struct EditParams {
    pub path: String,
    /// The exact text to replace: `old_text`, or `find` for `replace_all`.
    pub old_text: String,
    /// Its replacement: `new_text`, or `replace` for `replace_all`.
    pub new_text: String,
    /// Every match (`replace_all`) rather than exactly one (`replace_unique`).
    pub replace_all: bool,
    /// Compute the edit and show its diff without writing anything.
    pub dry_run: bool,
}

impl EditParams {
    /// The parameter names the agent used, for the messages.
    fn names(&self) -> (&'static str, &'static str) {
        if self.replace_all {
            ("find", "replace")
        } else {
            ("old_text", "new_text")
        }
    }
}

struct ReplaceArgs<'a> {
    content: &'a str,
    old_str: &'a str,
    new_str: &'a str,
    occurrences: usize,
    old_tokens: usize,
    new_tokens: usize,
}

/// Bounded, secret-redacted unified diff for edit evidence. `pub(crate)` so the
/// anchored editor (`sobre_patch`) reuses the identical evidence format.
pub fn build_diff_evidence(old: &str, new: &str, label: &str, max_lines: usize) -> String {
    let diff = similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(label, label)
        .to_string();
    // Single source of truth for secret masking — `core::redaction` carries the
    // non-secret-literal guard (type annotations, `undefined`, …) and
    // does not leak the value of generic long secrets like this duplicate once
    // did. Keeping a second regex set here only invites drift.
    let diff = crate::core::redaction::redact_text(&diff);

    let mut out = String::new();
    for (i, line) in diff.lines().enumerate() {
        if i >= max_lines {
            out.push_str(&format!("\n... diff truncated (max_lines={max_lines})"));
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end_matches('\n').to_string()
}

/// A cache mutation that an edit needs *after* its disk I/O completes.
///
/// Decoupling the cache mutation from the I/O lets the MCP layer perform the
/// (slow) file read/replace/write while holding only a cheap per-file lock, then
/// touch the shared cache for a sub-millisecond instant — instead of holding the
/// global cache write-lock across all disk I/O (the root cause of a contention bug).
pub enum CacheEffect {
    /// No cache change required (e.g. the edit failed before writing).
    None,
    /// The file on disk changed; drop the stale cache entry.
    Invalidate,
    /// Auto-escalation re-read full content that should be stored and marked
    /// as fully delivered.
    StoreFull(String),
}

/// Quality loop: classify the edit result and feed it into
/// [`crate::core::edit_quality`]. Only two outcomes carry a compression
/// signal: a clean replacement (success) and an `old_text` miss
/// (failure — the body the agent quoted wasn't what's on disk). Parameter
/// mistakes (empty/identical strings, missing file) and already-applied
/// edits say nothing about the read mode and are skipped.
pub fn record_outcome(params: &EditParams, last_mode: &str, text: &str, effect: &CacheEffect) {
    let success = matches!(effect, CacheEffect::Invalidate);
    let not_found = format!("ERROR: {} not found", params.names().0);
    let not_found_failure = matches!(effect, CacheEffect::StoreFull(_))
        || (matches!(effect, CacheEffect::None)
            && text.starts_with(&not_found)
            && !text.contains("already"));
    if success || not_found_failure {
        crate::core::edit_quality::record_edit_outcome(&params.path, last_mode, success);
    }
}

/// Applies a deferred [`CacheEffect`] to the session cache.
pub fn apply_cache_effect(cache: &mut SessionCache, path: &str, effect: CacheEffect) {
    match effect {
        CacheEffect::None => {}
        CacheEffect::Invalidate => {
            cache.invalidate(path);
        }
        CacheEffect::StoreFull(content) => {
            cache.store(path, &content);
            cache.mark_full_delivered(path);
        }
    }
}

/// Performs the full edit on disk **without** touching the session cache, and
/// reports back the [`CacheEffect`] the caller should apply afterwards.
///
/// `last_mode` is the cache's recorded read mode for the path (used only to
/// decide whether to auto-escalate on a not-found match); pass `""` when unknown.
pub fn run_io(params: &EditParams, last_mode: &str) -> (String, CacheEffect) {
    let file_path = &params.path;
    let (old_name, new_name) = params.names();

    let cap = crate::core::limits::max_read_bytes();
    let path = Path::new(file_path);
    let pre = match read_preimage(path, cap) {
        Ok(p) => p,
        Err(e) => {
            // File missing? Tell the agent whether it moved or the path is
            // wrong, instead of a bare "cannot open" (point 3).
            if !path.exists() {
                let hint = crate::tools::edit_recovery::moved_or_deleted_hint(path);
                return (format!("{e}{hint}"), CacheEffect::None);
            }
            return (e, CacheEffect::None);
        }
    };
    let content = &pre.text;

    if params.old_text.is_empty() {
        return (
            format!("ERROR: {old_name} must not be empty (op=create writes a new file)"),
            CacheEffect::None,
        );
    }

    if params.old_text == params.new_text {
        return (
            format!("ERROR: {old_name} and {new_name} are identical — nothing to change."),
            CacheEffect::None,
        );
    }

    let uses_crlf = pre.uses_crlf;
    let old_str = &params.old_text;
    let new_str = &params.new_text;
    let old_tokens = count_tokens(old_str);
    let new_tokens = count_tokens(new_str);

    let occurrences = content.matches(old_str.as_str()).count();

    if occurrences > 0 {
        let args = ReplaceArgs {
            content,
            old_str,
            new_str,
            occurrences,
            old_tokens,
            new_tokens,
        };
        return do_replace(path, &pre, params, cap, &args);
    }

    if uses_crlf && !old_str.contains('\r') {
        let old_crlf = old_str.replace('\n', "\r\n");
        let occ = content.matches(&old_crlf).count();
        if occ > 0 {
            let new_crlf = new_str.replace('\n', "\r\n");
            let args = ReplaceArgs {
                content,
                old_str: &old_crlf,
                new_str: &new_crlf,
                occurrences: occ,
                old_tokens,
                new_tokens,
            };
            return do_replace(path, &pre, params, cap, &args);
        }
    } else if !uses_crlf && old_str.contains("\r\n") {
        let old_lf = old_str.replace("\r\n", "\n");
        let occ = content.matches(&old_lf).count();
        if occ > 0 {
            let new_lf = new_str.replace("\r\n", "\n");
            let args = ReplaceArgs {
                content,
                old_str: &old_lf,
                new_str: &new_lf,
                occurrences: occ,
                old_tokens,
                new_tokens,
            };
            return do_replace(path, &pre, params, cap, &args);
        }
    }

    if content.contains(new_str.as_str()) {
        return (
            format!(
                "ERROR: {old_name} not found in {file_path}, but {new_name} already exists in the file. \
                 The edit was likely already applied (by a previous tool call or another agent)."
            ),
            CacheEffect::None,
        );
    }

    let preview = if old_str.len() > 80 {
        format!("{}...", &old_str[..old_str.floor_char_boundary(77)])
    } else {
        old_str.clone()
    };
    let hint = if uses_crlf {
        " (file uses CRLF line endings)"
    } else {
        ""
    };

    let closest_hint = find_closest_line_hint(content, old_str, old_name);
    let cross_file = crate::tools::edit_recovery::cross_file_hint(path, old_str);

    let (escalation, effect) = auto_escalate_reread(last_mode, file_path);

    (
        format!(
            "ERROR: {old_name} not found in {file_path}{hint}. \
             Make sure it matches exactly (including whitespace/indentation).\n\
             Searched for: {preview}{closest_hint}{cross_file}{escalation}"
        ),
        effect,
    )
}

/// Finds the closest matching line in the file content to help the agent
/// understand what went wrong. Returns a hint string or empty if no useful match.
fn find_closest_line_hint(content: &str, old_str: &str, old_name: &str) -> String {
    let first_line = old_str.lines().next().unwrap_or("").trim();
    if first_line.len() < 4 {
        return String::new();
    }

    let mut best_line: Option<(usize, &str)> = None;

    for (i, line) in content.lines().enumerate() {
        if line.contains(first_line) {
            best_line = Some((i + 1, line));
            break;
        }
    }

    // Try matching with significant identifiers from old_text's first line
    if best_line.is_none() {
        let keyword = first_line
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .find(|w| w.len() >= 4);

        if let Some(keyword) = keyword {
            for (i, line) in content.lines().enumerate() {
                if line.contains(keyword) {
                    best_line = Some((i + 1, line));
                    break;
                }
            }
        }
    }

    match best_line {
        Some((line_num, line_content)) => {
            let preview = if line_content.len() > 100 {
                format!("{}…", &line_content[..line_content.floor_char_boundary(97)])
            } else {
                line_content.to_string()
            };
            format!(
                "\nClosest match at line {line_num}, quoted with its whitespace (\\t is a tab): {preview:?}\n\
                 Hint: {old_name} must match exactly, indentation and trailing spaces included."
            )
        }
        None => String::new(),
    }
}

/// Auto-escalation: when old_text is not found and the file was previously read
/// in a compressed mode, re-read it and return the content so the agent can
/// immediately retry with the correct old_text. Returns the text to append
/// plus the [`CacheEffect`] the caller should apply: the full content is
/// recorded as delivered only when all of it was sent, so a later full read
/// of a long file is not answered with an "unchanged" stub.
fn auto_escalate_reread(last_mode: &str, path: &str) -> (String, CacheEffect) {
    if last_mode.is_empty() || last_mode == "full" {
        return (String::new(), CacheEffect::None);
    }

    let Ok(fresh_content) = std::fs::read_to_string(path) else {
        return (String::new(), CacheEffect::None);
    };

    let line_count = fresh_content.lines().count();
    const MAX_LINES: usize = 300;

    if line_count <= MAX_LINES {
        return (
            format!(
                "\n\n[auto-escalation] Last read used mode=\"{last_mode}\". \
                 Full content ({line_count}L) below — retry edit with exact text from here:\n\n{fresh_content}"
            ),
            CacheEffect::StoreFull(fresh_content),
        );
    }
    let lines: Vec<&str> = fresh_content.lines().collect();
    let head = &lines[..MAX_LINES / 2];
    let tail = &lines[line_count - MAX_LINES / 2..];
    let omitted = line_count - MAX_LINES;
    (
        format!(
            "\n\n[auto-escalation] Last read used mode=\"{last_mode}\". \
             The first and last {} of {line_count} lines below ({omitted} omitted; \
             sobre_read the rest) — retry edit with exact text from here:\n\n{}\n\
             [... {omitted} lines omitted ...]\n{}",
            MAX_LINES / 2,
            head.join("\n"),
            tail.join("\n")
        ),
        CacheEffect::None,
    )
}

fn do_replace(
    path: &Path,
    pre: &FilePreimage,
    params: &EditParams,
    cap: usize,
    args: &ReplaceArgs<'_>,
) -> (String, CacheEffect) {
    if args.occurrences > 1 && !params.replace_all {
        return (
            format!(
                "ERROR: old_text found {} times in {}. \
                 Provide more context to make old_text unique, or use op=replace_all.",
                args.occurrences,
                path.display()
            ),
            CacheEffect::None,
        );
    }

    let new_content = if params.replace_all {
        args.content.replace(args.old_str, args.new_str)
    } else {
        args.content.replacen(args.old_str, args.new_str, 1)
    };

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let syntax_notice =
        crate::core::syntax_validate::syntax_notice(ext, args.content, &new_content);

    if params.dry_run {
        let mut out = dry_run_text(path, args.content, &new_content);
        append_notice(&mut out, syntax_notice);
        return (out, CacheEffect::None);
    }

    // Code-health gate: an advisory notice on cognitive-complexity drift.
    let health_notice = crate::core::code_health::gate::evaluate(args.content, &new_content, ext);

    // A point-in-time check, not a held lock — see
    // ensure_preimage_still_matches' doc for the residual window between
    // this check and the write below.
    if let Err(e) = ensure_preimage_still_matches(path, &pre.fp, cap) {
        return (e, CacheEffect::None);
    }

    if let Err(e) =
        write_atomic_bytes_with_permissions(path, new_content.as_bytes(), Some(&pre.permissions))
    {
        return (e, CacheEffect::None);
    }

    if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
        bt.record_edit(&params.path);
    }

    let old_lines = args.content.lines().count();
    let new_lines = new_content.lines().count();
    let line_delta = new_lines as i64 - old_lines as i64;
    let delta_str = if line_delta > 0 {
        format!("+{line_delta}")
    } else {
        format!("{line_delta}")
    };

    let old_tokens = args.old_tokens;
    let new_tokens = args.new_tokens;

    let replaced_str = if args.occurrences > 1 {
        format!("{} replacements", args.occurrences)
    } else {
        "1 replacement".into()
    };

    let short = path.file_name().map_or_else(
        || path.to_string_lossy().to_string(),
        |f| f.to_string_lossy().to_string(),
    );

    let mut out =
        format!("✓ {short}: {replaced_str}, {delta_str} lines ({old_tokens}→{new_tokens} tok)");
    append_evidence(&mut out, args.content, &new_content, &short);
    append_notice(&mut out, syntax_notice);
    append_notice(&mut out, health_notice);
    (out, CacheEffect::Invalidate)
}

/// Appends the edit's diff (redacted, bounded) to a result.
pub(crate) fn append_evidence(out: &mut String, old: &str, new: &str, label: &str) {
    let diff = build_diff_evidence(old, new, label, DIFF_MAX_LINES);
    out.push_str("\n\nevidence (diff, redacted, bounded):\n```diff\n");
    out.push_str(&diff);
    out.push_str("\n```");
}

/// Appends an advisory notice (syntax, code health) to a result.
pub(crate) fn append_notice(out: &mut String, notice: Option<String>) {
    if let Some(notice) = notice {
        out.push_str("\n\n");
        out.push_str(&notice);
    }
}

/// What a dry run returns: the diff the edit would make. Nothing is written.
pub(crate) fn dry_run_text(path: &Path, old: &str, new: &str) -> String {
    let short = path.file_name().map_or_else(
        || path.to_string_lossy().to_string(),
        |f| f.to_string_lossy().to_string(),
    );
    let diff = build_diff_evidence(old, new, &short, DIFF_MAX_LINES);
    format!(
        "DRY RUN, nothing written to {}:\n```diff\n{diff}\n```",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    /// The three-call sequence production runs inline (`run_io` ->
    /// `record_outcome` -> `apply_cache_effect`). It used to be a shipped free
    /// function whose only callers were the tests below, so it is test
    /// scaffolding now rather than dead production code. It must stay in step
    /// with `registered::sobre_patch::replace_text`, which runs the same three
    /// calls for real.
    fn handle(cache: &mut SessionCache, params: &EditParams) -> String {
        let last_mode = cache
            .get(&params.path)
            .map(|e| e.last_mode.clone())
            .unwrap_or_default();
        let (text, effect) = run_io(params, &last_mode);
        record_outcome(params, &last_mode, &text, &effect);
        apply_cache_effect(cache, &params.path, effect);
        text
    }
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn mk_params(path: &Path, old: &str, new: &str, replace_all: bool) -> EditParams {
        EditParams {
            path: path.to_string_lossy().to_string(),
            old_text: old.to_string(),
            new_text: new.to_string(),
            replace_all,
            dry_run: false,
        }
    }

    #[test]
    fn replace_single_occurrence() {
        let f = make_temp("fn hello() {\n    println!(\"hello\");\n}\n");
        let mut cache = SessionCache::new();
        let result = handle(&mut cache, &mk_params(f.path(), "hello", "world", false));
        assert!(result.contains("ERROR"), "should fail: 'hello' appears 2x");
    }

    #[test]
    fn replace_all() {
        let f = make_temp("aaa bbb aaa\n");
        let mut cache = SessionCache::new();
        let result = handle(&mut cache, &mk_params(f.path(), "aaa", "ccc", true));
        assert!(result.contains("2 replacements"));
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(content, "ccc bbb ccc\n");
    }

    #[test]
    fn not_found_error() {
        let f = make_temp("some content\n");
        let mut cache = SessionCache::new();
        let result = handle(&mut cache, &mk_params(f.path(), "nonexistent", "x", false));
        assert!(result.contains("ERROR: old_text not found"));
    }

    // Parent dir read-only, file inode writable (the bind-mount
    // sandbox shape). The atomic tempfile + rename needs *directory* write
    // permission and fails; the in-place fallback overwrites the existing inode
    // and succeeds. Skipped under root, which bypasses the directory permission
    // check (the atomic path would then succeed and the fallback never runs —
    // the write still lands correctly either way).
    #[cfg(unix)]
    #[test]
    fn write_falls_back_on_readonly_parent_dir() {
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: geteuid() takes no arguments and only reads the caller's uid.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.jsonc");
        std::fs::write(&path, b"hello").unwrap();

        // r-x parent: create_new tempfile + rename fail with EACCES, but the
        // existing file mode (0o644) still allows O_WRONLY|O_TRUNC.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let res = write_atomic_bytes_with_permissions(&path, b"world", None);

        // Restore so tempdir cleanup can remove the directory.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(res.is_ok(), "in-place fallback should succeed: {res:?}");
        assert_eq!(std::fs::read(&path).unwrap(), b"world");
    }

    // End-to-end: the full text-replacement flow (read -> preimage -> write)
    // must succeed when the parent dir is read-only but the file is writable.
    #[cfg(unix)]
    #[test]
    fn handle_edit_succeeds_on_readonly_parent_dir() {
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: geteuid() takes no arguments and only reads the caller's uid.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.jsonc");
        std::fs::write(&path, "hello world\n").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let mut cache = SessionCache::new();
        let result = handle(&mut cache, &mk_params(&path, "hello", "goodbye", false));

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            result.contains('✓'),
            "edit should succeed via in-place fallback: {result}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye world\n");
    }

    #[test]
    fn unique_match_succeeds() {
        let f = make_temp("fn main() {\n    let x = 42;\n}\n");
        let mut cache = SessionCache::new();
        let result = handle(
            &mut cache,
            &mk_params(f.path(), "let x = 42", "let x = 99", false),
        );
        assert!(result.contains("✓"));
        assert!(result.contains("1 replacement"));
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("let x = 99"));
    }

    #[test]
    fn crlf_file_with_lf_search() {
        let f = make_temp("line1\r\nline2\r\nline3\r\n");
        let mut cache = SessionCache::new();
        let result = handle(
            &mut cache,
            &mk_params(f.path(), "line1\nline2", "changed1\nchanged2", false),
        );
        assert!(result.contains("✓"), "CRLF fallback should work: {result}");
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(
            content.contains("changed1\r\nchanged2"),
            "new_text should be adapted to CRLF: {content:?}"
        );
        assert!(
            content.contains("\r\nline3\r\n"),
            "rest of file should keep CRLF: {content:?}"
        );
    }

    #[test]
    fn lf_file_with_crlf_search() {
        let f = make_temp("line1\nline2\nline3\n");
        let mut cache = SessionCache::new();
        let result = handle(
            &mut cache,
            &mk_params(f.path(), "line1\r\nline2", "a\r\nb", false),
        );
        assert!(result.contains("✓"), "LF fallback should work: {result}");
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(
            content.contains("a\nb"),
            "new_text should be adapted to LF: {content:?}"
        );
    }

    /// Decision 2: no whitespace-insensitive match. The error shows the
    /// nearest line quoted, trailing spaces visible, and nothing is written.
    #[test]
    fn trailing_whitespace_is_not_ignored() {
        for (content, old, new) in [
            (
                "  let x = 1;  \n  let y = 2;\n",
                "  let x = 1;\n  let y = 2;",
                "  let x = 10;\n  let y = 20;",
            ),
            (
                "  const a = 1;  \r\n  const b = 2;\r\n",
                "  const a = 1;\n  const b = 2;",
                "  const a = 10;\n  const b = 20;",
            ),
        ] {
            let f = make_temp(content);
            for replace_all in [false, true] {
                let mut cache = SessionCache::new();
                let result = handle(&mut cache, &mk_params(f.path(), old, new, replace_all));
                assert!(result.starts_with("ERROR"), "{result}");
                assert!(
                    result.contains("1;  \""),
                    "the nearest line must show its trailing spaces: {result}"
                );
                assert_eq!(std::fs::read_to_string(f.path()).unwrap(), content);
            }
        }
    }

    #[test]
    fn rejects_invalid_utf8() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xff, 0xfe, 0xfd]).unwrap();
        let mut cache = SessionCache::new();
        let result = handle(&mut cache, &mk_params(f.path(), "a", "b", false));
        assert!(
            result.contains("not valid UTF-8"),
            "expected utf8 rejection, got: {result}"
        );
    }

    #[test]
    fn evidence_diff_is_emitted() {
        let f = make_temp("line1\nline2\n");
        let mut cache = SessionCache::new();
        let out = handle(&mut cache, &mk_params(f.path(), "line2", "changed2", false));
        assert!(out.contains("```diff"), "expected diff fence, got: {out}");
    }

    /// Run_io performs the full edit without any cache handle, so the
    /// MCP layer can avoid holding the global cache write-lock across disk I/O.
    /// A successful edit reports an Invalidate effect.
    #[test]
    fn run_io_success_reports_invalidate_effect() {
        let f = make_temp("fn main() {\n    let x = 42;\n}\n");
        let (text, effect) = run_io(&mk_params(f.path(), "let x = 42", "let x = 99", false), "");
        assert!(text.contains("✓"), "expected success: {text}");
        assert!(
            matches!(effect, CacheEffect::Invalidate),
            "successful edit must invalidate the cache entry"
        );
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("let x = 99"));
    }

    #[test]
    fn run_io_failure_reports_no_cache_effect() {
        let f = make_temp("some content\n");
        let (text, effect) = run_io(&mk_params(f.path(), "nonexistent", "x", false), "");
        assert!(text.contains("ERROR: old_text not found"));
        assert!(
            matches!(effect, CacheEffect::None),
            "a failed edit must not mutate the cache"
        );
    }

    /// Concurrent edits to *different* files must all succeed without
    /// serializing on any shared lock — run_io takes no cache, so there is nothing
    /// global to contend on.
    #[test]
    fn run_io_concurrent_edits_to_different_files_all_succeed() {
        use std::sync::Arc;
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let n = 16;
        let mut paths = Vec::new();
        for i in 0..n {
            let p = dir.path().join(format!("file_{i}.txt"));
            std::fs::write(&p, format!("value = {i}\n")).unwrap();
            paths.push(p);
        }
        let barrier = Arc::new(std::sync::Barrier::new(n));
        let mut handles = Vec::new();
        for (i, p) in paths.into_iter().enumerate() {
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let (text, effect) = run_io(
                    &mk_params(
                        &p,
                        &format!("value = {i}"),
                        &format!("value = {}", i + 1000),
                        false,
                    ),
                    "",
                );
                assert!(text.contains("✓"), "edit {i} failed: {text}");
                assert!(matches!(effect, CacheEffect::Invalidate));
                (p, i)
            }));
        }
        for h in handles {
            let (p, i) = h.join().unwrap();
            let content = std::fs::read_to_string(&p).unwrap();
            assert_eq!(content, format!("value = {}\n", i + 1000));
        }
    }

    #[test]
    fn run_io_escalation_reports_store_full_effect() {
        // A file previously read in a compressed mode ("signatures") triggers
        // auto-escalation when old_text is not found: the full content is
        // returned for re-store.
        let f = make_temp("line a\nline b\nline c\n");
        let (text, effect) = run_io(
            &mk_params(f.path(), "definitely-not-present", "x", false),
            "signatures",
        );
        assert!(
            text.contains("[auto-escalation]"),
            "expected escalation: {text}"
        );
        match effect {
            CacheEffect::StoreFull(content) => {
                assert!(content.contains("line a") && content.contains("line c"));
            }
            _ => panic!("escalation must report a StoreFull cache effect"),
        }
    }

    /// A long file escalates to its first and last lines only, so it is
    /// not recorded as delivered whole.
    #[test]
    fn a_partial_escalation_stores_nothing() {
        let body = (1..=400)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let f = make_temp(&body);
        let (text, effect) = run_io(
            &mk_params(f.path(), "definitely-not-present", "x", false),
            "signatures",
        );
        assert!(text.contains("100 lines omitted"), "{text}");
        assert!(
            matches!(effect, CacheEffect::None),
            "a partial view is not a full read"
        );
    }

    #[test]
    fn apply_cache_effect_invalidate_and_store() {
        let f = make_temp("hello\n");
        let mut cache = SessionCache::new();
        cache.store(&f.path().to_string_lossy(), "hello\n");
        apply_cache_effect(
            &mut cache,
            &f.path().to_string_lossy(),
            CacheEffect::Invalidate,
        );
        assert!(
            cache.get(&f.path().to_string_lossy()).is_none(),
            "Invalidate must drop the entry"
        );
        apply_cache_effect(
            &mut cache,
            &f.path().to_string_lossy(),
            CacheEffect::StoreFull("fresh\n".to_string()),
        );
        assert!(
            cache.get(&f.path().to_string_lossy()).is_some(),
            "StoreFull must re-populate the entry"
        );
    }

    #[test]
    fn identical_old_new_rejected() {
        let f = make_temp("fn main() {}\n");
        let mut cache = SessionCache::new();
        let result = handle(
            &mut cache,
            &mk_params(f.path(), "fn main() {}", "fn main() {}", false),
        );
        assert!(result.contains("identical"));
    }

    #[test]
    fn edit_already_applied_detected() {
        let f = make_temp("fn updated() {}\n");
        let (text, effect) = run_io(
            &mk_params(f.path(), "fn original() {}", "fn updated() {}", false),
            "",
        );
        assert!(text.contains("already exists"));
        assert!(text.contains("already applied"));
        assert!(matches!(effect, CacheEffect::None));
    }

    #[test]
    fn closest_line_hint_shown() {
        let f = make_temp("  fn hello() {\n    println!(\"hi\");\n  }\n");
        let (text, _) = run_io(
            &mk_params(f.path(), "fn hello(){", "fn hello_world(){", false),
            "",
        );
        assert!(text.contains("Closest match at line"));
    }

    #[test]
    fn missing_file_suggests_relocated_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/new")).unwrap();
        std::fs::write(dir.path().join("src/new/gizmo.rs"), "fn gizmo() {}\n").unwrap();

        let (text, effect) = run_io(
            &mk_params(
                &dir.path().join("src/old/gizmo.rs"),
                "fn gizmo() {}",
                "fn gizmo2() {}",
                false,
            ),
            "",
        );
        assert!(text.contains("same-named file was found"), "got: {text}");
        assert!(text.contains("gizmo.rs"), "got: {text}");
        assert!(matches!(effect, CacheEffect::None));
    }

    #[test]
    fn old_text_in_other_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let target = dir.path().join("a.rs");
        std::fs::write(&target, "fn unrelated_a() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn the_target_symbol() {}\n").unwrap();

        let (text, _) = run_io(
            &mk_params(
                &target,
                "fn the_target_symbol() {}",
                "fn renamed() {}",
                false,
            ),
            "",
        );
        assert!(text.contains("matching line exists in"), "got: {text}");
        assert!(text.contains("b.rs"), "got: {text}");
    }

    // P0-6: a symlink at the edit path is rejected: the atomic rename
    // would otherwise replace the link with a plain file.
    #[cfg(unix)]
    #[test]
    fn editing_through_a_symlink_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.rs");
        std::fs::write(&real, "fn old() {}\n").unwrap();
        let link = dir.path().join("link.rs");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let (text, effect) = run_io(&mk_params(&link, "fn old() {}", "fn new() {}", false), "");
        assert!(text.contains("symlink"), "got: {text}");
        assert!(matches!(effect, CacheEffect::None));
        // Target untouched.
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "fn old() {}\n");
    }

    #[test]
    fn regular_file_edit_still_works_after_symlink_guard() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("normal.rs");
        std::fs::write(&file, "fn old() {}\n").unwrap();

        let (text, _) = run_io(&mk_params(&file, "fn old() {}", "fn new() {}", false), "");
        assert!(
            text.contains("Edit applied") || !text.starts_with("ERROR"),
            "got: {text}"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn new() {}\n");
    }
}
