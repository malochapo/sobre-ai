use std::path::Path;

use crate::core::cache::SessionCache;
use crate::core::compressor;
use crate::core::deps;
use crate::core::entropy;
use crate::core::protocol;
use crate::core::signatures;
use crate::core::tokens::count_tokens;
use crate::tools::CrpMode;
mod helpers;
pub use helpers::is_instruction_file;
mod render;
pub(crate) use render::*;
/// Type-safe read-mode vocabulary: single source of truth for which
/// modes exist and how each is classified.
pub(crate) mod mode;
pub(crate) use mode::ReadMode;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_delta;
#[cfg(test)]
mod tests_windowed;

/// Pre-counted read output carrying the output string, resolved mode,
/// and token count computed during mode processing.
pub struct ReadOutput {
    pub content: String,
    pub resolved_mode: String,
    /// Approximate output token count from mode processing.
    /// The dispatch layer recounts the final assembled string for accurate savings.
    pub output_tokens: usize,
    /// Structurally determined cache-hit flag. Set by the code that
    /// serves the read (stub, delta, cached mode), not sniffed from rendered
    /// output. Replaces the fragile `content.contains("[unchanged")` checks.
    pub is_cache_hit: bool,
}

/// SSOT via [`ReadMode`]: the `map`/`signatures` summaries whose rendered
/// body is stored per-file in `compressed_outputs`. Unknown modes are not
/// cacheable, matching the prior `["map","signatures"].contains(mode)`.
pub(crate) fn is_cacheable_mode(mode: &str) -> bool {
    mode.parse::<ReadMode>()
        .is_ok_and(|m| m.is_compressed_cacheable())
}

/// Anti-inflation capping applies to whole-file views (`full` and the
/// lossy summaries `map`/`signatures`/`aggressive`/`entropy`/`task`/…), where the
/// raw file is a strict superset of the information and is therefore never a
/// worse answer when the framing happens to inflate on a small file. `full` is
/// included: an `auto` read can resolve to `full` and reach this path, and its
/// header must not push the cost above raw. Selection and delta views have
/// view-specific semantics — `lines:` returns a window, `reference` a pointer,
/// `diff` a delta, `raw` the bytes — so replacing them with the whole file would
/// be wrong, not cheaper, and they are never capped.
pub(crate) fn mode_allows_raw_cap(mode: &str) -> bool {
    // SSOT via [`ReadMode`]. Unknown modes keep the prior default of
    // `true` (only `lines:`/`reference`/`diff`/`raw` opt out of the cap).
    mode.parse::<ReadMode>()
        .map_or(true, |m| m.allows_raw_cap())
}

pub(crate) fn compressed_cache_key(
    mode: &str,
    crp_mode: CrpMode,
    task: Option<&str>,
    aggressiveness: Option<f64>,
    protect: &[String],
) -> String {
    let base = if crp_mode.is_tdd() {
        format!("{mode}:tdd")
    } else {
        mode.to_string()
    };
    // map/signatures output now embeds a task-relevant body, so task-aware and
    // task-free variants must cache under distinct keys.
    let keyed = match task.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            t.hash(&mut h);
            format!("{base}:t{:x}", h.finish())
        }
        None => base,
    };
    // Aggressiveness and the explicit protect list both change lossy output, so
    // both must change the key. Empty fragments keep pre-feature keys
    // byte-identical, so unmodified reads still hit their existing cache entries.
    let mut key = keyed;
    let aggr_frag = crate::core::aggressiveness::cache_fragment(aggressiveness);
    if !aggr_frag.is_empty() {
        key = format!("{key}:{aggr_frag}");
    }
    let protect_frag = crate::core::protect::protect_fragment(protect);
    if !protect_frag.is_empty() {
        key = format!("{key}:{protect_frag}");
    }
    key
}

/// Appends the reactive recovery footer to a compressed view, leading with the
/// MCP-free "read the path directly" route; the wording lives in
/// [`crate::core::recovery`]. Only lossy/compressed modes reach this helper, so
/// the footer is naturally absent from `full`/`raw`.
fn append_compressed_hint(output: &str, file_path: &str) -> String {
    format!(
        "{output}\n{}",
        crate::core::recovery::read_footer(file_path)
    )
}

/// Reads a file as UTF-8 with lossy fallback, enforcing binary detection and max read size limit.
pub fn read_file_lossy(path: &str) -> Result<String, std::io::Error> {
    if crate::core::binary_detect::is_binary_file(path) {
        let msg = crate::core::binary_detect::binary_file_message(path);
        return Err(std::io::Error::other(msg));
    }

    let cap = crate::core::limits::max_read_bytes();

    let file = open_with_retry(path)?;
    let meta = file
        .metadata()
        .map_err(|e| std::io::Error::other(format!("cannot stat open file descriptor: {e}")))?;
    if meta.len() > cap as u64 {
        return Err(std::io::Error::other(format!(
            "file too large ({} bytes, limit {} bytes): read it in windows, \
             mode=\"lines:1-200\" or start_line+limit",
            meta.len(),
            cap
        )));
    }

    use std::io::Read;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::io::BufReader::new(file).read_to_end(&mut bytes)?;
    let s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    // A UTF-8 BOM is an encoding artifact, not content — leaking it corrupts
    // the first line of every downstream view (a documented limitation).
    Ok(match s.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_owned(),
        None => s,
    })
}

/// A streamed line-window read: only the requested span's raw lines,
/// plus the file's true total line count for the header — the rest of the
/// file is never buffered.
struct LineWindow {
    /// Raw lines within `[start, end]`, joined with `\n`.
    body: String,
    /// True total line count of the file.
    total_lines: usize,
    /// Clamped, 1-based inclusive bounds actually served.
    start: usize,
    end: usize,
}

/// Parses an `anchored:` window payload for the disk-streaming short-circuit
/// below. Only the dash form (`"N-M"`) is fast-pathed — `anchored_lines_mode`
/// (the registered handler) always emits it, using the `999999` EOF sentinel
/// rather than a bare `"N"`. A hand-typed bare payload (meaning "N to EOF")
/// returns `None` and falls through to the normal full-read path instead of
/// guessing a total line count up front.
fn parse_disk_anchor_range(payload: &str) -> Option<(usize, usize)> {
    let (s, e) = payload.split_once('-')?;
    let start = s.trim().parse::<usize>().ok()?.max(1);
    let end = e.trim().parse::<usize>().ok()?;
    Some((start, end))
}

/// Streams `path` and keeps the lines inside any of `ranges` (1-based,
/// inclusive) with their numbers, plus the file's total line count, without
/// ever holding the whole file in memory. As in [`read_file_lossy`], a
/// leading BOM is dropped and an invalid UTF-8 byte becomes U+FFFD rather
/// than failing the read: a window is how a file over the size limit is
/// read, so it must not fail where the whole read would succeed. `None` on
/// an I/O error or a binary file.
fn read_line_ranges(
    path: &str,
    ranges: &[(usize, usize)],
) -> Option<(Vec<(usize, String)>, usize)> {
    if crate::core::binary_detect::is_binary_file(path) {
        return None;
    }
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(open_with_retry(path).ok()?);
    let mut buf = Vec::new();
    let mut total = 0usize;
    let mut kept = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf).ok()? == 0 {
            break;
        }
        total += 1;
        if ranges.iter().any(|&(s, e)| total >= s && total <= e) {
            let mut line: &[u8] = &buf;
            line = line.strip_suffix(b"\n").unwrap_or(line);
            line = line.strip_suffix(b"\r").unwrap_or(line);
            if total == 1 {
                line = line.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(line);
            }
            kept.push((total, String::from_utf8_lossy(line).into_owned()));
        }
    }
    Some((kept, total))
}

/// The single span `[start, end]` of `path`, streamed: the anchored-window
/// counterpart to [`read_file_lossy`].
fn read_line_window(path: &str, start: usize, end: usize) -> Option<LineWindow> {
    let (kept, total) = read_line_ranges(path, &[(start, end)])?;
    Some(LineWindow {
        body: kept
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n"),
        total_lines: total,
        start: start.min(total.max(1)),
        end: end.min(total),
    })
}

/// Attempt the disk-streaming short-circuit for a fresh `anchored:N-M`
/// read. `None` when the request isn't eligible (not a windowed anchored
/// read, or a preread is already in hand — nothing to short-circuit) or the
/// fast path can't run cleanly (binary file, invalid UTF-8, I/O error); the
/// caller falls through to the normal full-read path in that case.
fn try_disk_anchored_window(
    path: &str,
    mode: &str,
    fresh: bool,
    preread_is_none: bool,
    short: &str,
) -> Option<ReadOutput> {
    if !fresh || !preread_is_none {
        return None;
    }
    if let Some(range) = mode.strip_prefix("lines:") {
        return disk_lines_window(path, range, short);
    }
    let range = mode.strip_prefix("anchored:")?;
    let (start, end) = parse_disk_anchor_range(range)?;
    let window = read_line_window(path, start, end)?;
    let (out, _) = format_anchored_output_window(
        short,
        &window.body,
        window.total_lines,
        Some((window.start, window.end)),
    );
    let out = crate::core::redaction::redact_text(&out);
    let sent = count_tokens(&out);
    Some(ReadOutput {
        content: out,
        resolved_mode: mode.to_string(),
        output_tokens: sent,
        is_cache_hit: false,
    })
}

/// A `lines:` window of a file over the read size limit, streamed like an
/// anchored one: a window is the way to read such a file, so the limit must
/// not refuse it. Read like the in-memory `lines:` (`extract_line_range`):
/// comma-separated parts in the order given, `N` one line, `N-` to the end.
/// Smaller files take the normal, cached path.
fn disk_lines_window(path: &str, range: &str, short: &str) -> Option<ReadOutput> {
    let too_large = std::fs::metadata(path)
        .is_ok_and(|m| m.len() > crate::core::limits::max_read_bytes() as u64);
    if !too_large {
        return None;
    }
    let ranges = parse_line_ranges(range)?;
    let (kept, total) = read_line_ranges(path, &ranges)?;
    let mut selected = Vec::new();
    for &(start, end) in &ranges {
        for (n, line) in kept.iter().filter(|(n, _)| *n >= start && *n <= end) {
            selected.push(format!("{n:>4}| {line}"));
        }
    }
    let body = if selected.is_empty() {
        "No lines matched the range.".to_string()
    } else {
        selected.join("\n")
    };
    let out = format!("{short} {total}L lines:{range}\n{body}");
    let out = crate::core::redaction::redact_text(&out);
    let sent = count_tokens(&out);
    Some(ReadOutput {
        content: out,
        resolved_mode: format!("lines:{range}"),
        output_tokens: sent,
        is_cache_hit: false,
    })
}

/// `lines:` parts as 1-based inclusive spans; `None` when one does not parse.
fn parse_line_ranges(range: &str) -> Option<Vec<(usize, usize)>> {
    range
        .split(',')
        .map(|part| {
            let part = part.trim();
            if let Some((s, e)) = part.split_once('-') {
                let start = s.trim().parse::<usize>().ok()?.max(1);
                let end = if e.trim().is_empty() {
                    usize::MAX
                } else {
                    e.trim().parse::<usize>().ok()?
                };
                Some((start, end))
            } else {
                let n = part.parse::<usize>().ok()?;
                Some((n, n))
            }
        })
        .collect()
}

/// Opens a file, retrying once after a brief pause on NotFound.
/// Works around overlay/FUSE stat-cache races in container runtimes (Docker, Codex).
fn open_with_retry(path: &str) -> Result<std::fs::File, std::io::Error> {
    match std::fs::File::open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::File::open(path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    std::io::Error::other(format!(
                        "file not found: {path} — verify the path with sobre_tree or sobre_search"
                    ))
                } else {
                    e
                }
            })
        }
        Err(e) => Err(e),
    }
}

/// Reads a file through the cache and applies the requested compression mode.
pub fn handle(cache: &mut SessionCache, path: &str, mode: &str, crp_mode: CrpMode) -> String {
    handle_with_options(cache, path, mode, false, crp_mode, None)
}

/// Reads a file with task-aware filtering to prioritize task-relevant content.
pub fn handle_with_task(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    crp_mode: CrpMode,
    task: Option<&str>,
) -> String {
    handle_with_options(cache, path, mode, false, crp_mode, task)
}

/// Like `handle_with_task`, also returns the resolved mode name and pre-counted tokens.
#[cfg(any(test, feature = "test-support"))]
pub fn handle_with_task_resolved(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    crp_mode: CrpMode,
    task: Option<&str>,
) -> ReadOutput {
    handle_with_options_resolved(
        cache,
        path,
        mode,
        false,
        crp_mode,
        task,
        ReadTuning::resolve(None, &[]),
    )
}

/// Like `handle_with_task_resolved` but with an explicit per-call
/// aggressiveness (the `sobre_read` `aggressiveness` arg). `None` falls back
/// to the config field.
pub fn handle_with_task_resolved_tuned(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    crp_mode: CrpMode,
    task: Option<&str>,
    aggressiveness: Option<f64>,
    protect: &[String],
) -> ReadOutput {
    handle_with_options_resolved(
        cache,
        path,
        mode,
        false,
        crp_mode,
        task,
        ReadTuning::resolve(aggressiveness, protect),
    )
}

/// Fresh read with task-aware filtering (invalidates cache first).
pub fn handle_fresh_with_task(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    crp_mode: CrpMode,
    task: Option<&str>,
) -> String {
    handle_with_options(cache, path, mode, true, crp_mode, task)
}

/// Fresh read with task-aware filtering, also returns the resolved mode name and pre-counted tokens.
#[cfg(test)]
pub fn handle_fresh_with_task_resolved(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    crp_mode: CrpMode,
    task: Option<&str>,
) -> ReadOutput {
    handle_with_options_resolved(
        cache,
        path,
        mode,
        true,
        crp_mode,
        task,
        ReadTuning::resolve(None, &[]),
    )
}

/// Fresh-read variant of [`handle_with_task_resolved_tuned`].
pub fn handle_fresh_with_task_resolved_tuned(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    crp_mode: CrpMode,
    task: Option<&str>,
    aggressiveness: Option<f64>,
    protect: &[String],
) -> ReadOutput {
    handle_with_options_resolved(
        cache,
        path,
        mode,
        true,
        crp_mode,
        task,
        ReadTuning::resolve(aggressiveness, protect),
    )
}

fn handle_with_options(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    fresh: bool,
    crp_mode: CrpMode,
    task: Option<&str>,
) -> String {
    handle_with_options_resolved(
        cache,
        path,
        mode,
        fresh,
        crp_mode,
        task,
        ReadTuning::resolve(None, &[]),
    )
    .content
}

fn handle_with_options_resolved(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    fresh: bool,
    crp_mode: CrpMode,
    task: Option<&str>,
    tuning: ReadTuning<'_>,
) -> ReadOutput {
    handle_with_options_resolved_preread(cache, path, mode, fresh, crp_mode, task, tuning, None)
}

fn handle_with_options_resolved_preread(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    fresh: bool,
    crp_mode: CrpMode,
    task: Option<&str>,
    tuning: ReadTuning<'_>,
    preread: Option<String>,
) -> ReadOutput {
    if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
        bt.next_seq();
    }
    let mut result =
        handle_with_options_inner(cache, path, mode, fresh, crp_mode, task, tuning, preread);

    if let Some(entry) = cache.get_mut(path) {
        entry.last_mode.clone_from(&result.resolved_mode);
        // A partial/filtered read means the model's most recent view is NOT
        // the full content. Clear the delivery flag so a subsequent mode="full"
        // re-delivers real content instead of the [unchanged] stub. Without this,
        // a task→full sequence returns an empty stub because the flag was set by an
        // earlier full delivery and never cleared by the intervening non-full read.
        if !matches!(result.resolved_mode.as_str(), "full" | "full-compact") {
            entry.full_content_delivered = false;
        }
    }

    // SSOT via [`ReadMode`]: lossy summaries may elide shared blocks.
    let dedup_allowed = result
        .resolved_mode
        .parse::<ReadMode>()
        .is_ok_and(|m| m.is_lossy_summary());
    if dedup_allowed && let Some(deduped) = cache.apply_dedup(path, &result.content) {
        let new_tokens = count_tokens(&deduped);
        if new_tokens < result.output_tokens {
            result.content = deduped;
            result.output_tokens = new_tokens;
        }
    }

    if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
        let original_tokens = cache.get(path).map_or(0, |e| e.original_tokens);
        bt.record_read(
            path,
            &result.resolved_mode,
            result.output_tokens,
            original_tokens,
        );
    }

    result
}

/// Attempt to serve a `mode="full"` cache hit (`[unchanged …]`) using only a
/// shared borrow of the cache.
///
/// Returns `None` when the file is not cached, was modified on disk, full
/// content was never delivered, or the cache policy forbids stubbing — in those
/// cases the caller must fall back to the write path.
///
/// This is the read-locked fast path: it needs no `&mut SessionCache`, so the
/// dominant "re-read an unchanged file" case proceeds under a shared lock and
/// parallel reads of distinct files no longer serialize on a global write lock.
pub fn try_stub_hit_readonly(cache: &SessionCache, path: &str) -> Option<ReadOutput> {
    // Resolve the caller *fresh* (TTL-bypassed): the stub gate's concurrency
    // detection must see a just-appeared second chat with zero lag, else a stub
    // could leak across chats in the pre-detection window.
    let current_conversation = crate::core::conversation::current_conversation_id_fresh();
    try_stub_hit_readonly_scoped(cache, path, current_conversation.as_deref())
}

/// Conversation-scoped core of [`try_stub_hit_readonly`]. The current
/// conversation id is injected (not read from the global resolver) so the
/// conversation gate can be tested deterministically without global state.
pub(crate) fn try_stub_hit_readonly_scoped(
    cache: &SessionCache,
    path: &str,
    current_conversation: Option<&str>,
) -> Option<ReadOutput> {
    // Warm path: a live in-memory entry is the freshest source of truth.
    if let Some(file_ref) = cache.get_file_ref_readonly(path) {
        let (cached_mtime, cached_hash, line_count, delivered_conv) = {
            let entry = cache.get(path)?;
            (
                entry.stored_mtime,
                entry.hash.clone(),
                entry.line_count,
                entry.delivered_conversation.clone(),
            )
        };
        if crate::core::cache::is_cache_entry_stale_verified(path, cached_mtime, &cached_hash)
            || !cache.is_full_delivered(path)
        {
            return None;
        }
        // Conversation scoping: only stub when THIS conversation received
        // the content. A different (or unknown) conversation re-delivers in full
        // rather than emit a misleading stub. `current == None` (hooks absent)
        // preserves legacy process-scoped behavior, so single-chat hit rates are
        // unchanged.
        if !crate::core::conversation::conversation_allows_stub(
            current_conversation,
            delivered_conv.as_deref(),
        ) {
            return None;
        }
        cache.record_cache_hit(path);
        return Some(render_unchanged_stub(&file_ref, path, line_count));
    }

    // Cold fallback: no live entry (e.g. after an MCP restart or idle
    // clear). Serve the stub from the persisted index iff the file is unchanged
    // AND the *same known* conversation is asking — a stricter gate than the warm
    // path, because a cold stub crosses a process boundary (no "no context →
    // legacy" escape; see `conversation_allows_cold_stub`).
    let rec = crate::core::read_stub_index::lookup(path)?;
    if crate::core::cache::is_cache_entry_stale_verified(path, rec.stored_mtime(), &rec.hash) {
        return None;
    }
    if !crate::core::conversation::conversation_allows_cold_stub(
        current_conversation,
        rec.delivered_conversation.as_deref(),
    ) {
        return None;
    }
    Some(render_unchanged_stub(&rec.file_ref, path, rec.line_count))
}

/// Renders the `[unchanged …]` stub body shared by the warm and cold stub paths.
///
/// Determinism: the stub is a pure function of (file_ref, path, line_count),
/// so identical re-reads stay byte-stable and provider prompt caching applies.
/// The `fresh=true` escape is a *static* suffix (no rotating proof lines or
/// read-count notes), so a re-reader in non-meta mode still sees how to force the
/// content without breaking byte-stability.
fn render_unchanged_stub(file_ref: &str, path: &str, line_count: usize) -> ReadOutput {
    let short = protocol::shorten_path(path);
    let out = format!("{file_ref}={short} [unchanged {line_count}L · fresh=true to re-read]");
    let out = crate::core::redaction::redact_text(&out);
    let sent = count_tokens(&out);
    ReadOutput {
        content: out,
        resolved_mode: "full".into(),
        output_tokens: sent,
        is_cache_hit: true,
    }
}

/// Outcome of [`resolve_explicit_delta_mode`]: the (possibly rewritten) read
/// mode plus an optional advisory note to surface to the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaExplicitDecision {
    /// The mode the read should proceed with (rewritten only when the feature
    /// fires; otherwise the caller's mode, unchanged).
    pub mode: String,
    /// A byte-stable advisory appended to the read body when the mode was
    /// rewritten to `diff`. `None` when nothing was rewritten or the collapse
    /// was a silent `lines:`→`full` stub.
    pub note: Option<String>,
}

/// Decide whether an **explicit** `full`/`lines:N-M` re-read of a session-cached
/// file should be served as a delta instead of re-emitting content the model
/// already holds.
///
/// Returns the mode the read should proceed with:
/// - **Changed on disk** (verified mtime+md5 stale) and full content is cached →
///   `diff`, plus an advisory note. The diff carries exactly the new
///   information in a fraction of the tokens.
/// - **Unchanged** and the request is `lines:` of an already-fully-delivered
///   file → `full`, so the read collapses to the ~15-token `[unchanged]` stub
///   instead of re-extracting a window the model has seen.
/// - Otherwise the caller's `mode` is returned untouched.
///
/// First reads (nothing cached) and `fresh=true` are never affected — the
/// caller gates those before calling. Staleness uses the **verified** variant
/// ([`crate::core::cache::is_cache_entry_stale_verified`]) so a same-second
/// write on a coarse-granularity filesystem cannot be mistaken for "unchanged"
/// and yield a misleading empty diff (determinism).
///
/// Pure w.r.t. (cache, path, mode): no wall-clock, counters, or
/// randomness enter the result, so identical inputs stay byte-stable.
pub fn resolve_explicit_delta_mode(
    cache: &SessionCache,
    path: &str,
    mode: &str,
    explicit_mode: bool,
    fresh: bool,
) -> DeltaExplicitDecision {
    let unchanged = DeltaExplicitDecision {
        mode: mode.to_string(),
        note: None,
    };
    if fresh
        || !explicit_mode
        || !(mode == "full" || mode == "full-compact" || mode.starts_with("lines:"))
    {
        return unchanged;
    }
    let Some(entry) = cache.get(path) else {
        // First read this session — nothing to diff against.
        return unchanged;
    };
    let stale =
        crate::core::cache::is_cache_entry_stale_verified(path, entry.stored_mtime, &entry.hash);
    if stale {
        // Only divert to a diff when full content is actually cached: the diff
        // base is that full content (see `handle_diff`), never a compressed
        // view. Without it, `handle_diff` would have nothing to compare.
        if entry.content().is_some() {
            return DeltaExplicitDecision {
                mode: "diff".to_string(),
                note: Some(format!(
                    "[delta-explicit] requested mode={mode} served as a diff: the file \
                     changed since your last read and the diff is the new information. \
                     Pass fresh=true if you need the full content re-emitted."
                )),
            };
        }
        return unchanged;
    }
    // Unchanged on disk: a `lines:` window of a file already delivered in full
    // re-emits text the model holds — collapse to the full-mode stub
    // (~15 tokens). A plain `full` re-read already hits that stub downstream.
    if mode.starts_with("lines:") && cache.is_full_delivered(path) {
        return DeltaExplicitDecision {
            mode: "full".to_string(),
            note: None,
        };
    }
    unchanged
}

fn handle_with_options_inner(
    cache: &mut SessionCache,
    path: &str,
    mode: &str,
    fresh: bool,
    crp_mode: CrpMode,
    task: Option<&str>,
    tuning: ReadTuning<'_>,
    preread: Option<String>,
) -> ReadOutput {
    let file_ref = cache.get_file_ref(path);
    let short = protocol::shorten_path(path);
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    if fresh {
        if mode == "diff" {
            let warning = "[warning] fresh+diff is redundant — fresh invalidates cache, no diff possible. Use mode=full with fresh=true instead.";
            return ReadOutput {
                content: warning.to_string(),
                resolved_mode: "diff".into(),
                output_tokens: count_tokens(warning),
                is_cache_hit: false,
            };
        }
        cache.invalidate(path);
    }

    // A fresh, explicitly windowed `anchored:N-M` read never needs the
    // cache (fresh always bypasses it) or the whole file in memory — try the
    // disk-streaming short-circuit first.
    if let Some(out) = try_disk_anchored_window(path, mode, fresh, preread.is_none(), &short) {
        return out;
    }

    if mode == "diff" {
        let (out, _) = handle_diff(cache, path, &file_ref);
        let out = crate::core::redaction::redact_text(&out);
        let sent = count_tokens(&out);
        return ReadOutput {
            content: out,
            resolved_mode: "diff".into(),
            output_tokens: sent,
            is_cache_hit: false,
        };
    }

    if mode != "full"
        && let Some(existing) = cache.get(path)
    {
        let stale = crate::core::cache::is_cache_entry_stale_verified(
            path,
            existing.stored_mtime,
            &existing.hash,
        );
        if stale {
            cache.invalidate(path);
        }
    }

    // Snapshot the minimal immutable data the miss paths need, then drop the
    // borrow before any mutable operations (set_compressed, invalidate, store).
    let cache_snapshot = cache
        .get(path)
        .map(|existing| (existing.original_tokens, existing.content()));

    if let Some((original_tokens, content_opt)) = cache_snapshot {
        // Resolve the read mode first — and *cache-aware* for `auto`. Handing the
        // live cache to the resolver is what lets an `auto` re-read of an
        // unchanged, already-fully-delivered file short-circuit to
        // ("full", "cache_hit") and collapse to the cheap ~13-token `[unchanged]`
        // stub, exactly like an explicit `full` re-read. The previous call passed
        // no cache, so that branch was dead code and every `auto` re-read
        // re-delivered the whole file ("re-reads aren't cached"). Resolving
        // up-front also lets us hit the compressed-output cache BEFORE
        // decompressing the full body (avoids ~2-5ms zstd on hits). The
        // aggressiveness knob still routes `auto` through the density path.
        let resolved_mode = if mode == "auto" {
            tuning
                .auto_density_mode()
                .unwrap_or_else(|| resolve_auto_mode(Some(cache), path, original_tokens, task))
        } else {
            mode.to_string()
        };

        if resolved_mode == "full" || resolved_mode == "full-compact" {
            if let Some(out) = try_stub_hit_readonly(cache, path) {
                return out;
            }
            if resolved_mode == "full-compact" {
                let content = match read_file_lossy(path) {
                    Ok(c) => c,
                    Err(e) => {
                        let msg = format!("ERROR: {e}");
                        return ReadOutput {
                            content: msg,
                            resolved_mode: "error".into(),
                            output_tokens: 0,
                            is_cache_hit: false,
                        };
                    }
                };
                let (out, _) = format_full_compact_output(&content);
                let out = crate::core::redaction::redact_text(&out);
                let sent = count_tokens(&out);
                return ReadOutput {
                    content: out,
                    resolved_mode: "full-compact".into(),
                    output_tokens: sent,
                    is_cache_hit: false,
                };
            }
            let (out, _) = handle_full_with_auto_delta(cache, path, &file_ref, &short, ext, task);
            let out = crate::core::redaction::redact_text(&out);
            let sent = count_tokens(&out);
            return ReadOutput {
                content: out,
                resolved_mode: "full".into(),
                output_tokens: sent,
                is_cache_hit: false,
            };
        }

        if is_cacheable_mode(&resolved_mode) {
            let cache_key = compressed_cache_key(
                &resolved_mode,
                crp_mode,
                task,
                tuning.aggressiveness,
                tuning.protect,
            );
            let compressed_hit = cache.get_compressed(path, &cache_key).cloned();
            if let Some(cached_output) = compressed_hit {
                // get_compressed() already recorded the cache hit (stats + event)
                let out = crate::core::redaction::redact_text(&cached_output);
                let sent = count_tokens(&out);
                return ReadOutput {
                    content: out,
                    resolved_mode,
                    output_tokens: sent,
                    is_cache_hit: true,
                };
            }
        }

        if let Some(content) = content_opt {
            let (out, _) = process_mode_tuned(
                &content,
                &resolved_mode,
                &short,
                ext,
                original_tokens,
                crp_mode,
                path,
                task,
                tuning,
            );
            // Anti-inflation for lossy whole-file summaries (auto OR
            // explicit): map/signatures/… must never cost more than the raw file.
            // Selection/delta views keep their exact shape (see
            // mode_allows_raw_cap). Cap before caching so re-read hits serve the
            // same capped, byte-stable body.
            let out = if mode_allows_raw_cap(&resolved_mode) {
                let framed_tokens = count_tokens(&out);
                cap_to_raw(
                    out,
                    framed_tokens,
                    &content,
                    original_tokens,
                    &resolved_mode,
                )
            } else {
                out
            };
            if is_cacheable_mode(&resolved_mode) {
                let cache_key = compressed_cache_key(
                    &resolved_mode,
                    crp_mode,
                    task,
                    tuning.aggressiveness,
                    tuning.protect,
                );
                cache.set_compressed(path, &cache_key, out.clone());
            }
            let out = crate::core::redaction::redact_text(&out);
            let sent = count_tokens(&out);
            return ReadOutput {
                content: out,
                resolved_mode,
                output_tokens: sent,
                is_cache_hit: false,
            };
        }
        cache.invalidate(path);
    }

    // Two-Phase Read: when pre-read content was provided (disk I/O
    // already happened outside the cache lock), use it directly. Otherwise
    // fall back to reading from disk (legacy path, still used by fast-path
    // inline calls where the write lock was immediately available).
    let content = if let Some(pr) = preread {
        pr
    } else {
        match read_file_lossy(path) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("ERROR: {e}");
                let tokens = count_tokens(&msg);
                return ReadOutput {
                    content: msg,
                    resolved_mode: "error".into(),
                    output_tokens: tokens,
                    is_cache_hit: false,
                };
            }
        }
    };

    let store_result = cache.store(path, &content);

    // Skip expensive hint computation for line-range reads and first reads.
    // Hints are only useful from the 2nd read onwards when the file is contextually relevant.
    // sobre removed the TF-IDF semantic cache: the post-dispatch `[related: …]`
    // hint in `autonomy::enrich_after_read` already answers "what else should I
    // look at" from the code graph — and names only files that are *not* in the
    // session cache yet, which token overlap cannot do.

    if mode == "full" || mode == "full-compact" {
        cache.mark_full_delivered(path);

        if mode == "full-compact" {
            let (output, _) = format_full_compact_output(&content);
            let output = crate::core::redaction::redact_text(&output);
            let sent = count_tokens(&output);
            return ReadOutput {
                content: output,
                resolved_mode: "full-compact".into(),
                output_tokens: sent,
                is_cache_hit: false,
            };
        }

        let (output, _) = format_full_output(&short, ext, &content, store_result.line_count, task);
        let framed_tokens = count_tokens(&output);
        let output = cap_to_raw(
            output,
            framed_tokens,
            &content,
            store_result.original_tokens,
            "full",
        );
        let output = crate::core::redaction::redact_text(&output);
        let sent = count_tokens(&output);
        return ReadOutput {
            content: output,
            resolved_mode: "full".into(),
            output_tokens: sent,
            is_cache_hit: false,
        };
    }

    let resolved_mode = if mode == "auto" {
        tuning
            .auto_density_mode()
            .unwrap_or_else(|| resolve_auto_mode(None, path, store_result.original_tokens, task))
    } else {
        mode.to_string()
    };

    let (output, _sent) = process_mode_tuned(
        &content,
        &resolved_mode,
        &short,
        ext,
        store_result.original_tokens,
        crp_mode,
        path,
        task,
        tuning,
    );
    // Anti-inflation for lossy whole-file summaries (auto OR explicit);
    // selection/delta views keep their exact shape (see mode_allows_raw_cap).
    // Cap first, then cache the pure capped body so re-reads stay byte-stable
    // — the optional, read-state-dependent navigation hints below are
    // appended to the returned value only, never to the cached body.
    let output = if mode_allows_raw_cap(&resolved_mode) {
        let framed_tokens = count_tokens(&output);
        cap_to_raw(
            output,
            framed_tokens,
            &content,
            store_result.original_tokens,
            &resolved_mode,
        )
    } else {
        output
    };
    if is_cacheable_mode(&resolved_mode) {
        let cache_key = compressed_cache_key(
            &resolved_mode,
            crp_mode,
            task,
            tuning.aggressiveness,
            tuning.protect,
        );
        cache.set_compressed(path, &cache_key, output.clone());
    }
    let output = crate::core::redaction::redact_text(&output);
    let final_tokens = count_tokens(&output);
    ReadOutput {
        content: output,
        resolved_mode,
        output_tokens: final_tokens,
        is_cache_hit: false,
    }
}

/// Anti-inflation invariant: a `sobre_read` must never cost more tokens than
/// reading the raw file would. Framing (file-ref header, deps/exports summary,
/// savings footer, navigation hints) only earns its keep on large files and
/// repeated reads — on a cold read of a small file it is pure overhead, the
/// exact inflation an independent benchmark measured. When the framed
/// payload exceeds the bare content we ship the content verbatim, so a read is
/// break-even at worst and a win whenever a compressed mode or a cached re-read
/// applies. Re-reads are unaffected: the cache keys on path and re-derives the
/// file ref, so dropping the cold header here costs nothing on the next read.
///
/// `framed_tokens` and `raw_tokens` are both measured pre-redaction (redaction
/// is roughly token-neutral and applied to whichever string wins), so the
/// comparison is apples-to-apples with `original_tokens`. Empty files
/// (`raw_tokens == 0`) keep their framing so the reader still gets a signal.
///
/// A lossy `mode` replaced by the file says so in one line, so the agent knows
/// it got the whole file and not the view it asked for; a `full` view replaced
/// by the bare content needs no label.
pub(crate) fn cap_to_raw(
    framed: String,
    framed_tokens: usize,
    raw_content: &str,
    raw_tokens: usize,
    mode: &str,
) -> String {
    if raw_tokens > 0 && framed_tokens > raw_tokens {
        if matches!(mode, "full" | "full-compact") {
            raw_content.to_string()
        } else {
            format!("[{mode} view is larger than the file: whole file below]\n{raw_content}")
        }
    } else {
        framed
    }
}

/// Delegates to the unified `auto_mode_resolver::resolve()`.
/// Resolve `auto` to a concrete mode.
///
/// Pass `Some(cache)` on the warm read path: the resolver then short-circuits an
/// unchanged, already-fully-delivered file to `("full", "cache_hit")` so the
/// caller can collapse the re-read to the cheap `[unchanged]` stub instead of
/// re-delivering the whole body. Pass `None` only where no session cache exists
/// (the CLI cold path), which forces a stateless cold resolution.
pub(crate) fn resolve_auto_mode(
    cache: Option<&SessionCache>,
    file_path: &str,
    original_tokens: usize,
    task: Option<&str>,
) -> String {
    let ctx = crate::core::auto_mode_resolver::AutoModeContext {
        path: file_path,
        token_count: original_tokens,
        task,
        cache,
    };
    crate::core::auto_mode_resolver::resolve(&ctx).mode
}

const AUTO_DELTA_THRESHOLD: f64 = 0.6;

/// Re-reads from disk; if content changed and delta is compact, sends auto-delta.
fn handle_full_with_auto_delta(
    cache: &mut SessionCache,
    path: &str,
    file_ref: &str,
    short: &str,
    ext: &str,
    task: Option<&str>,
) -> (String, usize) {
    let Ok(disk_content) = read_file_lossy(path) else {
        cache.record_cache_hit(path);
        if let Some(existing) = cache.get(path) {
            if let Some(cached) = existing.content() {
                return format_full_output(short, ext, &cached, existing.line_count, task);
            }
            let out = format!(
                "[using cached version — file read failed]\n{file_ref}={short} cached {}t {}L",
                existing.read_count(),
                existing.line_count
            );
            let sent = count_tokens(&out);
            return (out, sent);
        }
        let out = format!("[file read failed and no cached version available] {short}");
        let sent = count_tokens(&out);
        return (out, sent);
    };

    let old_content = cache
        .get(path)
        .and_then(crate::core::cache::CacheEntry::content)
        .unwrap_or_default();
    let store_result = cache.store(path, &disk_content);

    if store_result.was_hit {
        // No stub here. Whether an unchanged file may collapse to
        // `[unchanged …]` is decided once, by `try_stub_hit_readonly`, which the
        // caller already consulted before routing here — and only that gate knows
        // whether THIS conversation received the content. A second
        // decision built from `StoreResult` cannot: `full_content_delivered` is
        // carried over from the cache entry, so it answers "some conversation got
        // this", which is the question the gate exists to stop trusting.
        cache.mark_full_delivered(path);
        return format_full_output(short, ext, &disk_content, store_result.line_count, task);
    }

    let diff = compressor::diff_content(&old_content, &disk_content);
    let diff_tokens = count_tokens(&diff);
    let full_tokens = store_result.original_tokens;

    if full_tokens > 0 && (diff_tokens as f64) < (full_tokens as f64 * AUTO_DELTA_THRESHOLD) {
        let head = short.to_string();
        let out = format!(
            "{head} [auto-delta] ∆{}L\n{diff}",
            disk_content.lines().count()
        );
        return (out, diff_tokens);
    }

    format_full_output(short, ext, &disk_content, store_result.line_count, task)
}

fn handle_diff(cache: &mut SessionCache, path: &str, file_ref: &str) -> (String, usize) {
    let short = protocol::shorten_path(path);
    let old_content = cache
        .get(path)
        .and_then(crate::core::cache::CacheEntry::content);

    let new_content = match read_file_lossy(path) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("ERROR: {e}");
            let tokens = count_tokens(&msg);
            return (msg, tokens);
        }
    };

    let diff_output = if let Some(old) = &old_content {
        compressor::diff_content(old, &new_content)
    } else {
        // No previous version cached — store content for future diffs but
        // return a short guidance message instead of dumping the full file.
        cache.store(path, &new_content);
        let msg = format!(
            "{file_ref}={short} [no cached version for diff — use mode=full first, then diff on re-read]"
        );
        let sent = count_tokens(&msg);
        return (msg, sent);
    };

    cache.store(path, &new_content);

    let sent = count_tokens(&diff_output);
    let head = short;
    (format!("{head} [diff]\n{diff_output}"), sent)
}
