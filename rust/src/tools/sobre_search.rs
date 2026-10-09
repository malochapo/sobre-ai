use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use glob::Pattern;
use ignore::WalkBuilder;
use regex::RegexBuilder;

use crate::core::protocol;
use crate::core::tokens::count_tokens;
use crate::tools::CrpMode;

pub(crate) const MAX_FILE_SIZE: u64 = 512_000;
pub(crate) const MAX_WALK_DEPTH: usize = 20;
const MAX_MATCH_LINE_WIDTH: usize = 150;

/// Result of a search: the rendered output and the size of what it replaced.
pub struct SearchOutcome {
    /// Rendered, compressed search output.
    pub text: String,
    /// Tokens measured in the raw match lines, the original the usage ledger
    /// compares the output against.
    pub observed_tokens: usize,
}

impl SearchOutcome {
    fn error(text: String) -> Self {
        Self {
            text,
            observed_tokens: 0,
        }
    }

    fn from_observed(text: String, observed_tokens: usize) -> Self {
        Self {
            text,
            observed_tokens,
        }
    }
}

/// Wall-clock budget for a single `sobre_search` call. The regular-file guard in
/// the read loop removes the known infinite block — `read_to_string` on a
/// FIFO/socket/device — while this deadline is the backstop for any
/// *other* pathological case (a gigantic corpus, a stuck network mount): the
/// tool returns partial results with a hint instead of appearing to hang.
const SEARCH_DEADLINE: Duration = Duration::from_secs(10);

/// Searches files for a regex pattern with compressed output and monorepo scope hints.
///
/// `anchored` (opt-in) appends a `:hh` line-hash to every match
/// (`path:line:hh content`) so a hit can be edited directly with `sobre_patch`
/// without a separate `sobre_read(mode="anchored")`. Default output is byte-for-byte
/// unchanged.
///
/// `exclude` (path glob, complement of `include`) and `exclude_pattern` (regex
/// on result lines, like `grep -v`) are the negative filters added.
#[allow(clippy::too_many_arguments)]
pub fn handle_filtered(
    pattern: &str,
    dir: &str,
    include: Option<&str>,
    max_results: usize,
    _crp_mode: CrpMode,
    respect_gitignore: bool,
    anchored: bool,
    exclude: Option<&str>,
    exclude_pattern: Option<&str>,
) -> SearchOutcome {
    // `include` is a glob matched against each file's path *relative to* `dir`
    // (e.g. `*.ts`, `*.{rs,ts}`, `src/**/*.tsx`). Bare globs without `/` match
    // at any directory depth (like `rg --glob`), so `*.ts` finds `a/b.ts` too.
    // Brace alternation is expanded here because the `glob` crate has no native
    // support for it. An empty result (no `include`, or only unparsable globs)
    // means "no filter", so a typo never silently drops every match.
    let include_patterns = compile_include(include);
    // `exclude` is the negative complement of `include`: a file matching any
    // exclude glob is dropped, even when it also matches `include`.
    let exclude_patterns = compile_include(exclude);
    const MAX_PATTERN_LEN: usize = 1024;
    const MAX_REGEX_SIZE: usize = 1 << 20; // 1 MiB DFA limit

    let redact = true;
    if pattern.len() > MAX_PATTERN_LEN {
        return SearchOutcome::error(format!(
            "ERROR: pattern too long ({} > {MAX_PATTERN_LEN} chars)",
            pattern.len()
        ));
    }
    let re = match RegexBuilder::new(pattern)
        .size_limit(MAX_REGEX_SIZE)
        .dfa_size_limit(MAX_REGEX_SIZE)
        .build()
    {
        Ok(r) => r,
        Err(e) => return SearchOutcome::error(format!("ERROR: invalid regex: {e}")),
    };
    // `exclude_pattern` drops any *result line* matching this regex — the
    // in-pipeline equivalent of `grep -v`. An unparsable pattern is ignored
    // (no exclusion), so a typo never silently hides every match.
    let exclude_re = exclude_pattern.and_then(|p| {
        RegexBuilder::new(p)
            .size_limit(MAX_REGEX_SIZE)
            .dfa_size_limit(MAX_REGEX_SIZE)
            .build()
            .ok()
    });

    let root = Path::new(dir);
    if !root.exists() {
        return SearchOutcome::error(format!("ERROR: {dir} does not exist"));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut matches = Vec::new();
    let mut raw_tokens_accum: usize = 0;
    let mut files_searched = 0u32;
    let mut files_skipped_size = 0u32;
    let mut files_skipped_encoding = 0u32;
    let mut files_skipped_special = 0u32;
    let mut files_skipped_type = 0u32;
    let mut deadline_hit = false;
    // Set when the search stopped at `max_results`: files were left unread,
    // or the last one read may hold more matches.
    let mut stopped_at_cap = false;
    // Set when any hit gained an `∈name@Lstart` enclosing tag, so the
    // self-describing legend is emitted only when it is actually needed.
    let mut any_enclosing = false;

    // Fast path: a warm resident trigram index narrows the candidate files in
    // memory, eliminating the per-call directory walk + full-corpus read. The
    // index covers the exact same file universe as the walk below, and matches
    // are still verified line-by-line with the same regex — so results are
    // identical. Missing/stale index → returns None and triggers a background
    // (re)build; this call uses the walk fallback.
    let used_index = if let Some(idx) = crate::core::search_index::get_fresh(dir, respect_gitignore)
    {
        files = idx
            .candidate_paths(pattern, &include_patterns, root)
            .into_paths();
        let skips = idx.skips();
        files_skipped_type += skips.by_type;
        files_skipped_size += skips.too_large;
        true
    } else {
        false
    };

    if !used_index {
        // Vendor dirs (node_modules, …) follow the gitignore toggle: explicitly
        // disabling gitignore is the escape hatch to look inside them.
        let walker = WalkBuilder::new(root)
            .hidden(true)
            .max_depth(Some(MAX_WALK_DEPTH))
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
            .build();

        for entry in walker.filter_map(std::result::Result::ok) {
            if entry.file_type().is_none_or(|ft| ft.is_dir()) {
                continue;
            }

            if entry.file_type().is_some_and(|ft| ft.is_symlink()) {
                continue;
            }

            let path = entry.path();

            // An explicitly requested file is always searched.
            if path != root && (is_binary_ext(path) || is_generated_file(path)) {
                files_skipped_type += 1;
                continue;
            }

            if !include_patterns.is_empty() {
                let rel = path.strip_prefix(root).unwrap_or(path);
                let rel_str = rel.to_string_lossy();
                if !include_patterns.iter().any(|p| glob_matches(p, &rel_str)) {
                    continue;
                }
            }

            // Size / regular-file filtering happens once in the shared read loop
            // below, so the walk path and the trigram-index fast path apply the
            // exact same eligibility rules.
            files.push(path.to_path_buf());
        }
    }

    // Drop excluded paths after both the index and walk populate `files`,
    // so the negative glob filter applies uniformly to either collection path.
    if !exclude_patterns.is_empty() {
        files.retain(|path| {
            let rel = path.strip_prefix(root).unwrap_or(path);
            let rel_str = rel.to_string_lossy();
            !exclude_patterns.iter().any(|p| glob_matches(p, &rel_str))
        });
    }

    // Deterministic search: stable file ordering makes max_results truncation reproducible.
    files.sort_unstable_by(|a, b| a.as_os_str().cmp(b.as_os_str()));

    let root_str = root.to_string_lossy();
    let deadline = Some(Instant::now() + SEARCH_DEADLINE);
    for path in &files {
        if matches.len() >= max_results {
            stopped_at_cap = true;
            break;
        }

        // Stop gracefully instead of appearing to hang on a pathological corpus
        // or a stuck read: once the wall-clock budget is spent, return
        // the partial results gathered so far with a hint to narrow the search.
        if deadline.is_some_and(|dl| Instant::now() >= dl) {
            deadline_hit = true;
            break;
        }

        // Only ever read regular files within the size budget. A FIFO, socket or
        // device node would block `read_to_string` forever — the root cause of
        // a hang — and oversized or unstatable files are skipped. `metadata`
        // (stat) never opens the file, so it cannot block on a special file.
        let state = match std::fs::metadata(path) {
            Ok(meta) if !meta.file_type().is_file() => {
                files_skipped_special += 1;
                continue;
            }
            Ok(meta) if meta.len() > MAX_FILE_SIZE => {
                files_skipped_size += 1;
                continue;
            }
            Ok(meta) => crate::core::content_cache::FileState::from_metadata(&meta),
            Err(_) => {
                files_skipped_encoding += 1;
                continue;
            }
        };

        // Reuse the copy the trigram-index build already read: the
        // corpus is read from disk once and the regex-verify pass here is an
        // in-memory hit. On a miss (cold cache / evicted) read once and publish
        // it for the next caller. `(mtime, size)` validation guarantees we never
        // verify against stale bytes.
        let content: std::sync::Arc<str> =
            if let Some(cached) = state.and_then(|s| crate::core::content_cache::get(path, s)) {
                cached
            } else {
                let Ok(text) = std::fs::read_to_string(path) else {
                    files_skipped_encoding += 1;
                    continue;
                };
                let arc: std::sync::Arc<str> = std::sync::Arc::from(text);
                if let Some(s) = state {
                    crate::core::content_cache::insert(path, s, std::sync::Arc::clone(&arc));
                }
                arc
            };

        files_searched += 1;
        // Enclosing-symbol spans for this file, computed lazily on the first hit
        // (never for non-matching files) and reused for every later hit here.
        let mut file_enclosing: Option<EnclosingIndex> = None;

        // A UTF-8 BOM is not part of line 1: an anchor hashed with it would
        // never match what `sobre_read` and `sobre_patch` hash.
        let body = content.strip_prefix('\u{feff}').unwrap_or(&content);
        for (i, line) in body.lines().enumerate() {
            // `exclude_pattern` filters out matching lines (grep -v).
            if re.is_match(line) && !exclude_re.as_ref().is_some_and(|ex| ex.is_match(line)) {
                let short_path =
                    protocol::shorten_path_relative(&path.to_string_lossy(), &root_str);
                // Count raw tokens incrementally (avoids separate Vec + join)
                raw_tokens_accum += count_tokens(line.trim()) + 2;
                let mut shown = if redact {
                    crate::core::redaction::redact_text(line.trim())
                } else {
                    line.trim().to_string()
                };
                if shown.len() > MAX_MATCH_LINE_WIDTH {
                    shown.truncate(shown.floor_char_boundary(MAX_MATCH_LINE_WIDTH));
                    shown.push_str("...");
                }
                // grep-ast enrichment: name the enclosing symbol + its
                // handle anchor so the hit is actionable without a follow-up
                // read. Appended after `shown`, so the `path:line content` prefix
                // every caller/test relies on is untouched.
                let tag = file_enclosing
                    .get_or_insert_with(|| EnclosingIndex::for_file(path, content.as_ref()))
                    .tag_for(i + 1);
                if tag.is_some() {
                    any_enclosing = true;
                }
                let tag = tag.unwrap_or_default();
                // The anchor hash is over the RAW line (matching sobre_read/sobre_patch
                // which both hash `content.lines()`), never the trimmed/truncated
                // display text — otherwise sobre_patch would always see a mismatch.
                if anchored {
                    matches.push(format!(
                        "{short_path}:{}:{} {}{}",
                        i + 1,
                        crate::core::anchor::line_hash(line),
                        shown,
                        tag
                    ));
                } else {
                    matches.push(format!("{short_path}:{} {}{}", i + 1, shown, tag));
                }
                if matches.len() >= max_results {
                    stopped_at_cap = true;
                    break;
                }
            }
        }
    }

    // Sort matches by file match-density — files with more hits surface
    // first, which is a proxy for relevance when the pattern contains common tokens.
    if matches.len() > 1 {
        use std::collections::HashMap;
        let mut file_counts: HashMap<String, usize> = HashMap::new();
        for m in &matches {
            let file = extract_file_from_match(m).to_string();
            *file_counts.entry(file).or_default() += 1;
        }
        matches.sort_by(|a, b| {
            let fa = extract_file_from_match(a);
            let fb = extract_file_from_match(b);
            let ca = file_counts.get(fa).copied().unwrap_or(0);
            let cb = file_counts.get(fb).copied().unwrap_or(0);
            cb.cmp(&ca).then_with(|| fa.cmp(fb))
        });
    }

    if matches.is_empty() {
        let mut msg = format!("0 matches for '{pattern}' in {files_searched} files");
        if files_skipped_size > 0 {
            msg.push_str(&format!(" ({files_skipped_size} large files skipped)"));
        }
        if files_skipped_encoding > 0 {
            msg.push_str(&format!(
                " ({files_skipped_encoding} files skipped: binary/encoding)"
            ));
        }
        if files_skipped_special > 0 {
            msg.push_str(&format!(
                " ({files_skipped_special} special files skipped: not regular files)"
            ));
        }
        if files_skipped_type > 0 {
            msg.push_str(&format!(" ({})", type_skip_note(files_skipped_type)));
        }
        msg.push_str(&format!(" ({})", not_searched_note(respect_gitignore)));
        if deadline_hit {
            msg.push_str(
                " (search stopped at the time budget — refine the pattern or scope with path=)",
            );
        }
        return SearchOutcome::error(msg);
    }

    // Prefix-cache-friendly: structural file list before per-query match content
    let matched_files: Vec<&str> = {
        let mut seen = HashSet::new();
        matches
            .iter()
            .filter_map(|m| {
                let file = extract_file_from_match(m);
                if seen.insert(file) { Some(file) } else { None }
            })
            .collect()
    };

    let mut result = format!(
        "{} matches in {} files ({files_searched} of {} files searched)",
        matches.len(),
        matched_files.len(),
        files.len()
    );
    if matched_files.len() > 1 {
        if matched_files.len() <= 10 {
            result.push_str(" [");
            result.push_str(&matched_files.join(", "));
            result.push(']');
        } else {
            let shown: Vec<&str> = matched_files.iter().take(8).copied().collect();
            result.push_str(&format!(
                " [{}, +{} more]",
                shown.join(", "),
                matched_files.len() - 8
            ));
        }
    }
    result.push_str(":\n");
    // Self-describing output: the anchor notation ships its own legend.
    if anchored {
        result.push_str("[anchored: path:line:hh → edit via sobre_patch]\n");
    }
    // Self-describing output: explain the `∈` enclosing tag and
    // how to turn it into a handle. Emitted only when at least one hit carries it.
    if any_enclosing {
        result.push_str(
            "[∈ enclosing symbol → sobre_search(action=symbol, handle=\"path#name@Lstart\")]\n",
        );
    }
    result.push_str(&matches.join("\n"));

    if files_skipped_size > 0 {
        result.push_str(&format!("\n({files_skipped_size} files >512KB skipped)"));
    }
    if files_skipped_encoding > 0 {
        result.push_str(&format!(
            "\n({files_skipped_encoding} files skipped: binary/encoding)"
        ));
    }
    if files_skipped_special > 0 {
        result.push_str(&format!(
            "\n({files_skipped_special} special files skipped: not regular files)"
        ));
    }
    if files_skipped_type > 0 {
        result.push_str(&format!("\n({})", type_skip_note(files_skipped_type)));
    }
    if deadline_hit {
        result.push_str(&format!(
            "\n(search stopped after the {}s budget — {files_searched} files scanned; \
             refine the pattern or scope with path= for full coverage)",
            SEARCH_DEADLINE.as_secs()
        ));
    }
    if stopped_at_cap {
        result.push_str(&format!(
            "\n(stopped at max_results={max_results}: more matches may exist — raise \
             max_results or narrow the pattern or path)"
        ));
    }
    result.push_str(&format!("\n({})", not_searched_note(respect_gitignore)));

    // Determinism contract: the hint must be a pure function of the
    // results. A show-once AtomicBool here made the first call differ from
    // every repeat, breaking byte-stability for provider prompt caches.
    let scope_hint = monorepo_scope_hint(&matches, dir);

    if let Some(hint) = scope_hint {
        result.push_str(&hint);
    }

    SearchOutcome::from_observed(result, raw_tokens_accum)
}

/// What a search or glob walk never looks at, said with every answer
/// (invariant 5: skipped files are named).
pub(crate) fn not_searched_note(respect_gitignore: bool) -> &'static str {
    if respect_gitignore {
        "not searched: hidden files, symlinks, .gitignore'd paths and node_modules"
    } else {
        "not searched: hidden files and symlinks"
    }
}

fn type_skip_note(count: u32) -> String {
    format!(
        "{count} files skipped by type: binary, minified, .lock, .map, .d.ts and similar; \
         pass path=<file> to search one"
    )
}

pub(crate) fn is_binary_ext(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "svg"
            | "woff"
            | "woff2"
            | "ttf"
            | "eot"
            | "pdf"
            | "zip"
            | "tar"
            | "gz"
            | "br"
            | "zst"
            | "bz2"
            | "xz"
            | "mp3"
            | "mp4"
            | "webm"
            | "ogg"
            | "wasm"
            | "so"
            | "dylib"
            | "dll"
            | "exe"
            | "lock"
            | "map"
            | "snap"
            | "patch"
            | "db"
            | "sqlite"
            | "parquet"
            | "arrow"
            | "bin"
            | "o"
            | "a"
            | "class"
            | "pyc"
            | "pyo"
    )
}

pub(crate) fn is_generated_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name.ends_with(".bundle.js")
        || name.ends_with(".chunk.js")
        || name.ends_with(".d.ts")
        || name.ends_with(".js.map")
        || name.ends_with(".css.map")
}

/// Per-file map from a line number to its narrowest enclosing symbol, built
/// once per matched file from the signature spans. Powers the `∈name@Lstart`
/// tag on each `sobre_search` hit (grep-ast pattern): the agent sees which
/// function/class a match lives in — and the handle to fetch it — without a
/// follow-up read. Tree-sitter-gated by construction: the regex-fallback
/// extractor yields single-line spans (`end == start`), which are filtered out
/// here, so a non-tree-sitter build emits byte-identical output.
struct EnclosingIndex {
    /// `(start_line, end_line, name)` for multi-line symbols, sorted by start.
    spans: Vec<(usize, usize, String)>,
}

impl EnclosingIndex {
    fn for_file(path: &Path, content: &str) -> Self {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mut spans: Vec<(usize, usize, String)> =
            crate::core::signatures::extract_signatures(content, ext)
                .into_iter()
                .filter_map(|s| match (s.start_line, s.end_line) {
                    (Some(a), Some(b)) if b > a => Some((a, b, s.name)),
                    _ => None,
                })
                .collect();
        spans.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.cmp(&y.1)));
        Self { spans }
    }

    /// The narrowest span containing `line`, rendered as the compact
    /// ` ∈name@Lstart` tag, or `None` when no multi-line symbol encloses it.
    fn tag_for(&self, line: usize) -> Option<String> {
        let mut best: Option<&(usize, usize, String)> = None;
        for sp in &self.spans {
            if line >= sp.0 && line <= sp.1 {
                match best {
                    None => best = Some(sp),
                    Some(b) if (sp.1 - sp.0) < (b.1 - b.0) => best = Some(sp),
                    _ => {}
                }
            }
        }
        best.map(|(start, _, name)| format!(" ∈{name}@L{start}"))
    }
}

/// Compile one glob as `rg --glob` reads it: a glob without `/` (`*.rs`,
/// `pathutil.rs`) names a file at any depth; a glob with `/` (`src/*.rs`,
/// `src/**/*.rs`) is matched against the path from the root.
pub(crate) fn compile_glob(glob: &str) -> Result<Pattern, glob::PatternError> {
    if glob.contains('/') {
        Pattern::new(glob)
    } else {
        Pattern::new(&format!("**/{glob}"))
    }
}

/// Whether `path` matches `pattern` as a shell glob does: `*` and `?` stay
/// within one path segment (`src/*.ts` is not `src/a/b.ts`); `**` crosses
/// segments.
pub(crate) fn glob_matches(pattern: &Pattern, path: &str) -> bool {
    pattern.matches_with(
        path,
        glob::MatchOptions {
            require_literal_separator: true,
            ..glob::MatchOptions::default()
        },
    )
}

/// Upper bound on the number of globs a single `include` may expand to, so a
/// pathological brace pattern (`{a,b}{c,d}{e,f}…`) can never blow up.
const MAX_INCLUDE_GLOBS: usize = 64;

/// Compile an `include` filter into one or more matchers.
///
/// Brace alternation (`*.{rs,ts}`) is expanded to multiple globs (`*.rs`,
/// `*.ts`) because the `glob` crate matches `{` / `}` literally. A file is
/// included when it matches *any* of the returned patterns. An empty vec means
/// "no filter": `include` was `None`, or every expansion failed to parse.
///
/// Each glob is read by [`compile_glob`].
fn compile_include(include: Option<&str>) -> Vec<Pattern> {
    let Some(raw) = include else {
        return Vec::new();
    };
    expand_braces(raw)
        .into_iter()
        .take(MAX_INCLUDE_GLOBS)
        .filter(|g| !g.is_empty())
        .filter_map(|g| compile_glob(&g).ok())
        .collect()
}

/// Expand one or more `{a,b,c}` brace groups into the cartesian set of concrete
/// globs. Patterns without braces (or with an unbalanced brace) are returned
/// unchanged, so this is safe to call on any input.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close_rel) = pattern[open..].find('}') else {
        return vec![pattern.to_string()];
    };
    let close = open + close_rel;
    let prefix = &pattern[..open];
    let inner = &pattern[open + 1..close];
    let suffix = &pattern[close + 1..];

    let mut out = Vec::new();
    for alt in inner.split(',') {
        let alt = alt.trim();
        for expanded_suffix in expand_braces(suffix) {
            out.push(format!("{prefix}{alt}{expanded_suffix}"));
            if out.len() >= MAX_INCLUDE_GLOBS {
                return out;
            }
        }
    }
    out
}

/// Extract file path from a grep match line, handling Windows drive letters (e.g. "C:").
fn extract_file_from_match(line: &str) -> &str {
    let start = if line.len() >= 2
        && line.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && line.as_bytes().get(1) == Some(&b':')
    {
        2
    } else {
        0
    };
    match line[start..].find(':') {
        Some(pos) => &line[..start + pos],
        None => line,
    }
}

fn monorepo_scope_hint(matches: &[String], search_dir: &str) -> Option<String> {
    let top_dirs: HashSet<&str> = matches
        .iter()
        .filter_map(|m| {
            let path = extract_file_from_match(m);
            let relative = path.strip_prefix("./").unwrap_or(path);
            let relative = relative.strip_prefix(search_dir).unwrap_or(relative);
            let relative = relative.strip_prefix('/').unwrap_or(relative);
            relative.split('/').next()
        })
        .collect();

    if top_dirs.len() > 3 {
        let mut dirs: Vec<&&str> = top_dirs.iter().collect();
        dirs.sort();
        let dir_list: Vec<String> = dirs.iter().take(6).map(|d| format!("'{d}'")).collect();
        let extra = if top_dirs.len() > 6 {
            format!(", +{} more", top_dirs.len() - 6)
        } else {
            String::new()
        };
        Some(format!(
            "\n\nResults span {} directories ({}{}). \
             Use the 'path' parameter to scope to a specific service, \
             e.g. path=\"{}/\".",
            top_dirs.len(),
            dir_list.join(", "),
            extra,
            dirs[0]
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::CrpMode;

    /// Determinism contract: identical search over identical files
    /// must produce byte-identical output — a prerequisite for provider
    /// prompt-cache hits on repeated tool results.
    #[test]
    fn search_output_is_byte_stable_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(
                dir.path().join(format!("f{i}.rs")),
                format!("fn target_{i}() {{}}\nfn other() {{}}\n"),
            )
            .unwrap();
        }
        let root = dir.path().to_string_lossy().into_owned();
        let run = || {
            handle_filtered(
                "target",
                &root,
                Some("*.rs"),
                20,
                CrpMode::Off,
                true,
                false,
                None,
                None,
            )
            .text
        };
        assert_eq!(run(), run(), "search output must be deterministic");
    }

    /// Line 1 of a file with a UTF-8 BOM is anchored without the BOM,
    /// as `sobre_read` and `sobre_patch` hash it.
    #[test]
    fn anchored_search_hashes_line_one_without_the_bom() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.rs"), "\u{feff}let needle = 1;\n").unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let out = handle_filtered(
            "needle",
            &root,
            Some("*.rs"),
            10,
            CrpMode::Off,
            true,
            true,
            None,
            None,
        )
        .text;
        let hh = crate::core::anchor::line_hash("let needle = 1;");
        assert!(out.contains(&format!("b.rs:1:{hh} ")), "{out}");
    }

    /// Opt-in anchored search tags each hit with `:hh` (matching
    /// `sobre_read`/`sobre_patch`'s line hash) and ships a legend; the default
    /// (anchored=false) output stays byte-identical (deterministic).
    #[test]
    fn anchored_search_emits_line_hash_per_hit_opt_in_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "let needle = 1;\nother\n").unwrap();
        let root = dir.path().to_string_lossy().into_owned();

        let plain = handle_filtered(
            "needle",
            &root,
            Some("*.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(
            !plain.contains("[anchored:"),
            "default must carry no legend"
        );
        assert!(
            plain.contains("a.rs:1 "),
            "default keeps path:line content: {plain}"
        );

        let anchored = handle_filtered(
            "needle",
            &root,
            Some("*.rs"),
            10,
            CrpMode::Off,
            true,
            true,
            None,
            None,
        )
        .text;
        let hh = crate::core::anchor::line_hash("let needle = 1;");
        assert!(anchored.contains("[anchored: path:line:hh → edit via sobre_patch]"));
        assert!(
            anchored.contains(&format!("a.rs:1:{hh} ")),
            "anchored hit must carry the line hash: {anchored}"
        );
    }

    #[test]
    #[cfg(feature = "tree-sitter")]
    fn hits_inside_multiline_symbols_carry_enclosing_tag() {
        // A hit inside a multi-line function names its enclosing symbol +
        // the handle anchor, and the output ships a self-describing legend.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn outer() {\n    let needle = 1;\n    needle\n}\nfn tiny() {}\n",
        )
        .unwrap();
        let out = handle_filtered(
            "needle",
            dir.path().to_string_lossy().as_ref(),
            Some("*.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(
            out.contains("∈outer@L1"),
            "hit must name its enclosing fn: {out}"
        );
        assert!(
            out.contains("[∈ enclosing symbol"),
            "self-describing legend must be present: {out}"
        );
    }

    #[test]
    fn single_line_symbols_get_no_enclosing_tag() {
        // A match whose only enclosing symbol is single-line gets no
        // tag, so the default output stays byte-identical (no `∈`, no legend).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one_liner() {}\n").unwrap();
        let out = handle_filtered(
            "one_liner",
            dir.path().to_string_lossy().as_ref(),
            Some("*.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(!out.contains('∈'), "single-line symbol → no tag: {out}");
    }

    #[test]
    fn search_results_are_deterministically_ordered_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&b, "match\n").unwrap();
        std::fs::write(&a, "match\n").unwrap();

        let out = handle_filtered(
            "match",
            dir.path().to_string_lossy().as_ref(),
            Some("*.txt"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;

        let mut match_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.contains(".txt:") && l.contains("match"))
            .collect();
        // Expect exactly the 2 match lines, ordered a.txt then b.txt.
        match_lines.truncate(2);
        assert_eq!(match_lines.len(), 2);
        assert!(
            match_lines[0].contains("a.txt:"),
            "first match should come from a.txt, got: {}",
            match_lines[0]
        );
        assert!(
            match_lines[1].contains("b.txt:"),
            "second match should come from b.txt, got: {}",
            match_lines[1]
        );
    }

    #[test]
    fn warm_index_and_content_cache_path_returns_correct_matches() {
        // Exercises the trigram-index fast path together with the shared content
        // cache: the index build reads the corpus once and publishes it,
        // then this search reuses those bytes. Results must be byte-identical to
        // the walk path — this asserts that correctness, independent of whether
        // any individual file is a cache hit or a fallback re-read.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn authenticate() {}\nlet x = 1;\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn connect() {}\n").unwrap();
        let root = dir.path().to_string_lossy().to_string();

        // Synchronously warm the resident trigram index (also populates the
        // shared content cache for these paths).
        assert!(
            crate::core::search_index::warm_blocking(&root, true),
            "index should warm for a small clean corpus"
        );

        let out = handle_filtered(
            "authenticate",
            &root,
            None,
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(
            out.contains("a.rs"),
            "warm-index + cache search must find the match: {out}"
        );
        assert!(
            out.contains("authenticate"),
            "the matched line must be present: {out}"
        );
        assert!(
            !out.contains("b.rs"),
            "a non-matching file must not appear in results: {out}"
        );
    }

    #[test]
    fn search_finds_word_literals_added_after_index_warm() {
        // Regression: a native edit or add after the trigram index was
        // warmed must be found immediately. The resident index is gated by a live
        // corpus signature, so stale trigrams can never hide on-disk content from
        // a word-literal query — the very path that uses trigram narrowing.
        let _lock = crate::core::data_dir::test_env_lock();

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn existing() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn other() {}\n").unwrap();
        let root = dir.path().to_string_lossy().to_string();

        assert!(
            crate::core::search_index::warm_blocking(&root, true),
            "index should warm for a small clean corpus"
        );

        // (1) A brand-new file created after the warm.
        std::fs::write(dir.path().join("c.rs"), "fn added_after_warm_zzz() {}\n").unwrap();
        let out_new = handle_filtered(
            "added_after_warm_zzz",
            &root,
            None,
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(
            out_new.contains("c.rs") && out_new.contains("added_after_warm_zzz"),
            "a file created after the index warm must be found: {out_new}"
        );

        // (2) An in-place edit of a file that existed at warm time.
        std::fs::write(
            dir.path().join("a.rs"),
            "fn existing() {}\nfn edited_after_warm_zzz() {}\n",
        )
        .unwrap();
        let out_edit = handle_filtered(
            "edited_after_warm_zzz",
            &root,
            None,
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(
            out_edit.contains("a.rs") && out_edit.contains("edited_after_warm_zzz"),
            "content appended after the index warm must be found: {out_edit}"
        );
    }

    #[test]
    fn symbol_substitution_is_off_by_default() {
        let _lock = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.rs");
        std::fs::write(
            &f,
            "fn longIdentifierAlpha() {}\nfn longIdentifierBeta() {}\nfn longIdentifierGamma() {}\n",
        )
        .unwrap();

        let out = handle_filtered(
            "longIdentifier",
            dir.path().to_string_lossy().as_ref(),
            Some("*.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;

        assert!(
            !out.contains("§MAP"),
            "default agent-facing output must not carry a §MAP table: {out}"
        );
        assert!(
            !out.contains('α'),
            "default agent-facing output must not carry α-symbols: {out}"
        );
        assert!(
            out.contains("longIdentifierAlpha"),
            "identifiers should appear raw by default: {out}"
        );
    }

    /// A search that stops at `max_results` says so and counts against
    /// every candidate file; a complete one does not, and both name what the
    /// walk never looks at.
    #[test]
    fn stopping_at_max_results_is_said() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            let body = (0..10)
                .map(|j| format!("needle {i} {j}"))
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(dir.path().join(format!("f{i:02}.txt")), body).unwrap();
        }
        let search = |max| {
            handle_filtered(
                "needle",
                dir.path().to_string_lossy().as_ref(),
                None,
                max,
                CrpMode::Off,
                true,
                false,
                None,
                None,
            )
            .text
        };
        let capped = search(20);
        assert!(capped.contains("of 30 files searched"), "{capped}");
        assert!(capped.contains("stopped at max_results=20"), "{capped}");
        assert!(capped.contains("not searched: hidden files"), "{capped}");
        let all = search(1000);
        assert!(
            all.starts_with("300 matches in 30 files (30 of 30 files searched)"),
            "{all}"
        );
        assert!(!all.contains("stopped at max_results"), "{all}");
        assert!(all.contains("not searched: hidden files"), "{all}");
    }

    #[test]
    fn type_skips_are_announced_and_explicit_files_searched() {
        let dir = tempfile::tempdir().unwrap();
        let min = dir.path().join("app.min.js");
        std::fs::write(&min, "needle\n").unwrap();
        std::fs::write(dir.path().join("ok.txt"), "needle\n").unwrap();

        let out = handle_filtered(
            "needle",
            dir.path().to_string_lossy().as_ref(),
            None,
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(out.contains("ok.txt"), "got: {out}");
        assert!(!out.contains("app.min.js:"), "got: {out}");
        assert!(out.contains("1 files skipped by type"), "got: {out}");

        let out = handle_filtered(
            "needle",
            min.to_string_lossy().as_ref(),
            None,
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(
            out.contains("needle"),
            "explicit file must be searched: {out}"
        );
        assert!(!out.contains("0 matches"), "got: {out}");
    }

    #[test]
    #[cfg(unix)]
    fn search_skips_named_pipe_without_hanging() {
        use std::sync::mpsc;
        // A named pipe (FIFO) in the search universe used to block
        // `read_to_string` forever, hanging the whole call with no output. It
        // must be skipped, the real file still matched, and the call must return.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real.txt"), "needle_here = 1\n").unwrap();
        let fifo = dir.path().join("pipe.fifo");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(
            // SAFETY: `c` is a live CString providing a valid NUL-terminated
            // path pointer for the duration of the call.
            unsafe { libc::mkfifo(c.as_ptr(), 0o644) },
            0,
            "mkfifo failed"
        );

        let dir_path = dir.path().to_string_lossy().to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Fresh temp dir → no warm index yet, so this exercises the walk path.
            let out = handle_filtered(
                "needle_here",
                &dir_path,
                None,
                10,
                CrpMode::Off,
                true,
                false,
                None,
                None,
            )
            .text;
            let _ = tx.send(out);
        });
        let out = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("sobre_search hung on a FIFO (regression)");

        assert!(
            out.contains("real.txt"),
            "the real file must still match: {out}"
        );
        assert!(
            out.contains("special files skipped"),
            "the FIFO must be reported as a skipped special file: {out}"
        );
    }

    #[test]
    fn repeating_a_search_returns_the_full_result_again() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("x.rs"), "repeat_needle_q7\n").unwrap();
        std::fs::write(b.path().join("y.rs"), "repeat_needle_q7\n").unwrap();
        let run = |dir: &std::path::Path| {
            handle_filtered(
                "repeat_needle_q7",
                dir.to_string_lossy().as_ref(),
                None,
                10,
                CrpMode::Off,
                true,
                false,
                None,
                None,
            )
            .text
        };
        let first = run(a.path());
        assert_eq!(first, run(a.path()), "same search, same output");
        assert!(
            run(b.path()).contains("y.rs"),
            "another dir is searched in full"
        );
    }

    #[test]
    fn include_glob_filters_by_brace_expansion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "brace_glob_needle\n").unwrap();
        std::fs::write(dir.path().join("b.ts"), "brace_glob_needle\n").unwrap();
        std::fs::write(dir.path().join("c.py"), "brace_glob_needle\n").unwrap();

        let out = handle_filtered(
            "brace_glob_needle",
            dir.path().to_string_lossy().as_ref(),
            Some("*.{rs,ts}"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;

        assert!(out.contains("a.rs"), "rs file must match: {out}");
        assert!(out.contains("b.ts"), "ts file must match: {out}");
        assert!(!out.contains("c.py"), "py file must be excluded: {out}");
    }

    #[test]
    fn bare_include_glob_matches_at_any_depth() {
        // rg/git grep behaviour: a bare glob without `/` should match
        // files at any depth, not just in the search root.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/deep/path")).unwrap();
        std::fs::write(dir.path().join("a/deep/path/file.rs"), "needle\n").unwrap();
        std::fs::write(dir.path().join("root.rs"), "needle\n").unwrap();
        std::fs::write(dir.path().join("other.py"), "needle\n").unwrap();

        let out = handle_filtered(
            "needle",
            dir.path().to_string_lossy().as_ref(),
            Some("*.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;

        assert!(out.contains("root.rs"), "root .rs file must match: {out}");
        assert!(
            out.contains("file.rs"),
            "nested .rs file must match bare *.rs glob: {out}"
        );
        assert!(!out.contains("other.py"), ".py must be excluded: {out}");

        // Also test bare filename glob (no wildcard at all)
        let out2 = handle_filtered(
            "needle",
            dir.path().to_string_lossy().as_ref(),
            Some("file.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;

        assert!(
            out2.contains("file.rs"),
            "bare filename glob must match nested file: {out2}"
        );
    }

    #[test]
    fn include_glob_recursive_path_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/inner")).unwrap();
        std::fs::write(dir.path().join("src/inner/deep.rs"), "needle\n").unwrap();
        std::fs::write(dir.path().join("top.rs"), "needle\n").unwrap();

        let out = handle_filtered(
            "needle",
            dir.path().to_string_lossy().as_ref(),
            Some("src/**/*.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;

        assert!(out.contains("deep.rs"), "nested match expected: {out}");
        assert!(
            !out.contains("top.rs"),
            "root file outside src/ must be excluded: {out}"
        );
    }

    #[test]
    fn exclude_glob_drops_paths_and_exclude_pattern_drops_lines() {
        // `exclude` is the negative complement of `include` (path glob),
        // and `exclude_pattern` is a `grep -v` over result lines.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.rs"), "needle\n// needle skip me\n").unwrap();
        std::fs::write(dir.path().join("app_test.rs"), "needle\n").unwrap();
        let root = dir.path().to_string_lossy().into_owned();

        // exclude drops the *_test.rs file entirely.
        let out = handle_filtered(
            "needle",
            &root,
            None,
            10,
            CrpMode::Off,
            true,
            false,
            Some("*_test.rs"),
            None,
        )
        .text;
        assert!(out.contains("app.rs"), "app.rs must match: {out}");
        assert!(
            !out.contains("app_test.rs"),
            "excluded path must be dropped: {out}"
        );

        // exclude_pattern drops the commented match line but keeps the real one.
        let out2 = handle_filtered(
            "needle",
            &root,
            Some("app.rs"),
            10,
            CrpMode::Off,
            true,
            false,
            None,
            Some("skip me"),
        )
        .text;
        assert!(out2.contains("1 matches"), "one line kept: {out2}");
        assert!(!out2.contains("skip me"), "grep -v line dropped: {out2}");
    }

    #[test]
    fn invalid_exclude_pattern_is_ignored_not_fatal() {
        // A malformed exclude regex must not hide matches — it's ignored.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "needle\n").unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let out = handle_filtered(
            "needle",
            &root,
            None,
            10,
            CrpMode::Off,
            true,
            false,
            None,
            Some("([unclosed"),
        )
        .text;
        assert!(out.contains("a.rs"), "invalid exclude → no filter: {out}");
    }
}
