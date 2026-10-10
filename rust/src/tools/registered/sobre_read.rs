use std::sync::{Arc, Mutex};

use rmcp::ErrorData;
use rmcp::model::{ContentBlock, Tool};
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{
    McpTool, ToolContext, ToolOutput, get_bool, get_f64, get_int, get_str, get_str_array,
    require_resolved_path,
};
use crate::tool_defs::tool_def;

/// Per-file lock that serializes concurrent reads of the same path.
///
/// When multiple subagents read sequentially through a shared set of files,
/// they tend to hit the same path at the same time. Without per-file locking
/// they all contend on the global cache write lock while doing redundant I/O.
/// This lock ensures only one thread reads a given file from disk; the others
/// wait cheaply on the per-file mutex, then hit the warm cache.
///
/// Backed by the shared `core::path_locks` registry so reads and edits of the
/// same path coordinate through a single mutex.
fn per_file_lock(path: &str) -> Arc<Mutex<()>> {
    crate::core::path_locks::per_file_lock(path)
}

pub struct SobreReadTool;

impl McpTool for SobreReadTool {
    fn name(&self) -> &'static str {
        "sobre_read"
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_read",
            "Read source files. mode recommended — choose by intent (see `mode` below); defaults to auto when omitted.\n\
             To UNDERSTAND code run sobre_compose FIRST; sobre_read after it identified files.\n\
             anchored → edit by reference via sobre_patch (no exact-recall).",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path" },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Batch read" },
                    "mode": {
                        "type": "string",
                        "description": "Defaults to auto (sobre picks). full=whole file, edit-ready; secret-like values show as [REDACTED:…] anchored=full with N:hh anchors to edit via sobre_patch raw=the bytes on disk: no framing, no redaction signatures=API map=structure aggressive|entropy|task=lossy summaries diff=what changed since this session last read the file reference=one line: size and type lines:N-M=window (lines:N is one line, lines:N- runs to the end, lines:5,10-20 multi-selects) anchored:N-M=anchored window density:0.NN=target share of tokens"
                    },
                    "raw": { "type": "boolean", "description": "Same as mode=raw" },
                    "start_line": { "type": "integer", "description": "1-based" },
                    "offset": { "type": "integer", "description": "start_line alias" },
                    "limit": { "type": "integer", "description": "Max lines" },
                    "fresh": { "type": "boolean", "description": "Bypass cache" },
                    "aggressiveness": { "type": "number", "description": "0.0–1.0 density (entropy/task)" },
                    "protect": { "type": "array", "items": { "type": "string" }, "description": "Symbols kept verbatim" }
                },
                "required": []
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        // sobre_read absorbs multi-file batch reads.
        // A non-empty `paths` array routes to the one shared batch implementation.
        if args
            .get("paths")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty())
        {
            return super::batch_read::batch_read(args, ctx);
        }

        let path = require_resolved_path(ctx, args, "path")?;

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.handle_inner(args, ctx, &path)
        })) {
            Ok(result) => result,
            Err(_) => Err(ErrorData::internal_error(
                format!(
                    "sobre_read panicked while processing '{path}'. This is a bug — please report it."
                ),
                None,
            )),
        }
    }
}

impl SobreReadTool {
    #[allow(clippy::unused_self)]
    fn handle_inner(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
        path: &str,
    ) -> Result<ToolOutput, ErrorData> {
        let session_lock = ctx
            .session
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("session not available", None))?;
        let cache_lock = ctx
            .cache
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("cache not available", None))?;

        let current_task = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
            let mut attempt = 0u32;
            loop {
                // Use try_read_owned instead of Handle::block_on to avoid
                // the async-runtime-saturation anti-pattern on Windows.
                if let Ok(guard) = session_lock.clone().try_read_owned() {
                    break guard.task.as_ref().map(|t| t.description.clone());
                }
                attempt += 1;
                if std::time::Instant::now() >= deadline {
                    tracing::warn!(
                        "session read-lock timeout after {attempt} attempts in sobre_read for {path}"
                    );
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        };
        // `raw=true` is the intuitive "give me the exact bytes" escape an
        // agent reaches for. Alias it to mode="raw" (verbatim, unframed) and
        // force a fresh disk read below so a re-read never collapses to an
        // `[unchanged]`/auto-delta stub. An explicit raw flag wins over `mode`.
        let arg_raw = get_bool(args, "raw").unwrap_or(false);
        let explicit_mode_arg = resolve_raw_alias(arg_raw, get_str(args, "mode"));
        let explicit_mode = explicit_mode_arg.is_some();
        // An unknown or malformed mode fails loudly (`lines:44:48` with a
        // colon) instead of silently returning the whole file. A bare
        // `density:` stays valid: it takes the aggressiveness target.
        if let Some(ref requested) = explicit_mode_arg
            && requested != "density:"
            && let Err(e) = requested.parse::<crate::tools::sobre_read::ReadMode>()
        {
            return Err(ErrorData::invalid_params(e.user_message(), None));
        }
        let mut mode = if let Some(m) = explicit_mode_arg {
            m
        } else if let Ok(cache) = cache_lock.try_read() {
            crate::tools::impl_smart_read::select_mode(&cache, path)
        } else {
            tracing::debug!(
                "cache lock contested during auto-mode selection for {path}; \
                 falling back to full"
            );
            "full".to_string()
        };
        let mut fresh = get_bool(args, "fresh").unwrap_or(false);
        // A raw/verbatim request always reads from disk — the whole point
        // is exact current bytes, never a cached stub or delta.
        if arg_raw {
            fresh = true;
        }
        let aggressiveness =
            crate::core::aggressiveness::effective(get_f64(args, "aggressiveness"));
        let protect = get_str_array(args, "protect").unwrap_or_default();
        // One-knob UX: when the caller sets aggressiveness without pinning a mode,
        // route through the proven density path at the mapped target. An explicit
        // mode (incl. entropy/task) instead has the knob tune it via ReadTuning.
        if !explicit_mode && let Some(a) = aggressiveness {
            // SSOT mode construction via the typed `ReadMode`: the typed
            // `Density` Display emits the same `density:0.NN` the pipeline parses.
            mode = crate::tools::sobre_read::ReadMode::Density(
                crate::core::aggressiveness::AggressivenessProfile::from_level(a).density_target,
            )
            .to_string();
        }
        // `raw` is the bytes on disk: a line window of it is those lines as
        // they are, with no numbering, framing or redaction.
        if mode == "raw"
            && explicit_mode
            && let Some((start, limit)) = resolve_line_window(
                get_int(args, "start_line"),
                get_int(args, "offset"),
                get_int(args, "limit"),
            )
        {
            if crate::core::binary_detect::is_binary_file(path) {
                let msg = crate::core::binary_detect::binary_file_message(path);
                return Err(ErrorData::invalid_params(msg, None));
            }
            let start = usize::try_from(start).unwrap_or(1).max(1);
            let end = limit
                .and_then(|l| usize::try_from(l).ok())
                .map_or(usize::MAX, |l| start.saturating_add(l.max(1)) - 1);
            let content = crate::tools::sobre_read::raw_line_window(path, start, end)
                .map_err(|e| ErrorData::invalid_params(format!("{path}: {e}"), None))?;
            return Ok(ToolOutput::simple(content));
        }

        // `start_line` (and its `offset`/`limit` aliases) can pin a line window.
        // The resolution lives in `apply_line_window`/`resolve_line_window` so
        // the runtime path and the unit tests share one implementation and can
        // never drift (aliases, explicit mode, line 1).
        let mode_before_window = mode.clone();
        apply_line_window(
            &mut mode,
            &mut fresh,
            explicit_mode,
            get_int(args, "start_line"),
            get_int(args, "offset"),
            get_int(args, "limit"),
        );
        let windowed = mode != mode_before_window;

        let gate_result = crate::server::context_gate::pre_dispatch_read(path, &mode);
        // An explicit raw/verbatim request is never silently downgraded by
        // the budget gate — the caller asked for exact bytes.
        let mut mode_override_note: Option<String> = None;
        if mode != "raw"
            && let Some(overridden) = gate_result.overridden_mode
        {
            if explicit_mode {
                let reason = gate_result.reason.unwrap_or("context-gate");
                mode_override_note = Some(format!(
                    "[mode overridden: {mode} -> {overridden}, reason={reason}]"
                ));
            }
            mode = overridden;
        }

        // An instruction file (SKILL.md, AGENTS.md, rules) is read whole when
        // the agent asked for nothing in particular; a mode, a line window or
        // an aggressiveness it picked is honoured.
        let picked = explicit_mode || windowed || aggressiveness.is_some();
        let mut mode = if !picked && crate::tools::sobre_read::is_instruction_file(path) {
            "full".to_string()
        } else {
            mode
        };

        // Delta-aware explicit re-reads. Re-requesting full/lines:N-M content for
        // a file this session already read re-emits content the model already holds;
        // when the file changed on disk, a diff carries the same information in
        // a fraction of the tokens, and an unchanged lines: request of a
        // fully-delivered file collapses to the full-mode stub. The decision is
        // a pure function of (cache, path, mode) — see
        // `sobre_read::resolve_explicit_delta_mode`. First reads are unaffected;
        // fresh=true always bypasses. Runs BEFORE the lines:→fresh guard below
        // so a changed-file lines: re-read can still be diverted to a diff.
        // Compaction-aware: if the host compacted since the last check, reset
        // the delivery flags first, so neither a stub nor a delta is decided
        // on content the agent no longer holds.
        if !fresh
            && let Ok(data_dir) = crate::core::data_dir::sobre_data_dir()
            && let Ok(mut cache) = cache_lock.try_write()
        {
            crate::server::compaction_sync::sync_if_compacted(&mut cache, &data_dir);
        }

        let mut delta_explicit_note: Option<String> = None;
        if !fresh
            && explicit_mode
            && (mode == "full" || mode.starts_with("lines:"))
            && let Ok(cache) = cache_lock.try_read()
        {
            let decision = crate::tools::sobre_read::resolve_explicit_delta_mode(
                &cache,
                path,
                &mode,
                explicit_mode,
                fresh,
            );
            mode = decision.mode;
            delta_explicit_note = decision.note;
        }

        if mode.starts_with("lines:") {
            fresh = true;
        }

        if crate::core::binary_detect::is_llm_viewable_image(path) {
            return read_image_file(path);
        }
        if crate::core::binary_detect::is_binary_file(path) {
            let msg = crate::core::binary_detect::binary_file_message(path);
            return Err(ErrorData::invalid_params(msg, None));
        }
        // A file over the read limit is read in windows: `lines:N-M` and
        // `anchored:N-M` stream just the window, so only whole-file modes are
        // refused.
        let is_window = mode.starts_with("lines:") || mode.starts_with("anchored:");
        if !is_window {
            let cap = crate::core::limits::max_read_bytes() as u64;
            if let Ok(meta) = std::fs::metadata(path)
                && meta.len() > cap
            {
                let msg = format!(
                    "File too large to read whole ({} bytes, limit {} bytes). \
                     Read it in windows: mode=\"lines:1-200\" or start_line+limit, \
                     or mode=\"anchored\" with start_line+limit for edit-ready lines.",
                    meta.len(),
                    cap
                );
                return Err(ErrorData::invalid_params(msg, None));
            }
        }

        // `raw` is the file as it is on disk: no cache, no framing, no
        // redaction, nothing appended. Every other mode may shape the content
        // and says so.
        if mode == "raw" {
            let content = crate::tools::sobre_read::read_file_lossy(path)
                .map_err(|e| ErrorData::invalid_params(format!("{path}: {e}"), None))?;
            return Ok(ToolOutput::simple(content));
        }

        let (output, resolved_mode, original, file_ref) = read_through_cache(
            cache_lock,
            path,
            &mode,
            fresh,
            ctx.crp_mode,
            current_task.as_deref(),
            aggressiveness,
            &protect,
        )
        .map_err(|e| ErrorData::internal_error(e, None))?;

        if resolved_mode == "error" {
            return Err(ErrorData::invalid_params(output, None));
        }

        let output_tokens = crate::core::tokens::count_tokens(&output);
        // A line window replaces reading those lines, not the whole file: it
        // claims no saving (`usage.jsonl` would otherwise count the rest of
        // the file as saved).
        let replaced_tokens =
            if resolved_mode.starts_with("lines:") || resolved_mode.starts_with("anchored:") {
                output_tokens
            } else {
                original
            };
        let saved = replaced_tokens.saturating_sub(output_tokens);

        // Session updates (bounded lock — 10s timeout, read already succeeded)
        {
            let rt = tokio::runtime::Handle::current();
            let session_guard = rt.block_on(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                session_lock.write(),
            ));
            if let Ok(mut session) = session_guard {
                session.touch_file(path, file_ref.as_deref(), &resolved_mode, original);
                let file_summary = extract_file_summary(&output, path);
                if !file_summary.is_empty() {
                    session.set_file_summary(path, &file_summary);
                }
                let root_missing = session
                    .project_root
                    .as_deref()
                    .is_none_or(|r| r.trim().is_empty());
                if root_missing && let Some(root) = crate::core::protocol::detect_project_root(path)
                {
                    session.project_root = Some(root);
                }
            } else {
                tracing::warn!(
                    "session write-lock timeout (10s) in sobre_read post-update for {path}"
                );
            }
        }

        // Cross-source hints: the property graph's cross-source edges for this
        // file (the code-health hotspots) are appended as compact hints. Only
        // touch the DB when it already exists — never create graph.db on a read.
        let hints_suffix = {
            let graph_db = crate::core::property_graph::graph_dir(&ctx.project_root)
                .map(|d| d.join("graph.db"));
            let edges = if graph_db.is_some_and(|db| db.exists()) {
                crate::core::property_graph::CodeGraph::open(&ctx.project_root)
                    .map(|g| g.all_cross_source_edges())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if edges.is_empty() {
                String::new()
            } else {
                let hints = crate::core::cross_source_hints::hints_for_file(
                    path,
                    &edges,
                    &ctx.project_root,
                );
                crate::core::cross_source_hints::format_hints(&hints)
            }
        };

        let mut warnings = Vec::new();
        if let Some(ref w) = delta_explicit_note {
            warnings.push(w.as_str());
        }
        if let Some(ref w) = mode_override_note {
            warnings.push(w.as_str());
        }
        // Notices (mode override, delta) go BEFORE the payload so client-side
        // truncation of large outputs cannot hide them.
        let final_output = if !warnings.is_empty() {
            format!("{}\n\n{output}{hints_suffix}", warnings.join("\n"))
        } else if hints_suffix.is_empty() {
            output
        } else {
            format!("{output}{hints_suffix}")
        };

        Ok(ToolOutput {
            text: final_output,
            saved_tokens: saved,
            shell_outcome: None,
            content_blocks: None,
        })
    }
}

/// Waits up to `limit` for the cache's write lock.
fn write_lock_within(
    cache_lock: &Arc<tokio::sync::RwLock<crate::core::cache::SessionCache>>,
    limit: std::time::Duration,
) -> Option<tokio::sync::RwLockWriteGuard<'_, crate::core::cache::SessionCache>> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let Ok(guard) = cache_lock.try_write() {
            return Some(guard);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// A read through the session cache, one way whatever the contention:
/// - the `[unchanged]` stub under a shared lock (re-reads of distinct files
///   run in parallel);
/// - else the cache's part of the read under a brief write lock
///   ([`crate::tools::sobre_read::prepare`]);
/// - a compressed view rendered with no lock held, so a large file's render
///   never blocks another call;
/// - the result stored under a second brief lock, or returned uncached when
///   that lock does not come within 5 s.
///
/// Same-file reads are serialized by the per-file lock. Returns `(output,
/// resolved mode, original tokens, file ref)`; `Err` when a lock does not
/// come in time.
#[allow(clippy::too_many_arguments)]
fn read_through_cache(
    cache_lock: &Arc<tokio::sync::RwLock<crate::core::cache::SessionCache>>,
    path: &str,
    mode: &str,
    fresh: bool,
    crp_mode: crate::tools::CrpMode,
    task: Option<&str>,
    aggressiveness: Option<f64>,
    protect: &[String],
) -> Result<(String, String, usize, Option<String>), String> {
    use crate::tools::sobre_read::{
        Prepared, ReadTuning, finish, prepare, record_read, render, settle,
    };

    let file_lock = per_file_lock(path);
    let _file_guard = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        loop {
            if let Ok(guard) = file_lock.try_lock() {
                break guard;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "per-file lock contention for {path} — retry in a moment"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };

    if !fresh
        && matches!(mode, "full" | "auto")
        && let Ok(cache) = cache_lock.try_read()
        && let Some(out) = crate::tools::sobre_read::try_stub_hit_readonly(&cache, path)
    {
        let orig = cache.get(path).map_or(0, |e| e.original_tokens);
        let file_ref = cache.file_ref_map().get(path).cloned();
        return Ok((out.content, out.resolved_mode, orig, file_ref));
    }

    let tuning = ReadTuning::resolve(aggressiveness, protect);
    let contention = || format!("cache lock contention for {path} — retry in a moment");
    // The bounce tracker is a static lock: taken only with no cache lock held.
    if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
        bt.next_seq();
    }
    let job = {
        let mut cache = write_lock_within(cache_lock, std::time::Duration::from_secs(10))
            .ok_or_else(contention)?;
        match prepare(&mut cache, path, mode, fresh, crp_mode, task, tuning, None) {
            Prepared::Done(out) => {
                let out = settle(&mut cache, path, out);
                let orig = cache.get(path).map_or(0, |e| e.original_tokens);
                let file_ref = cache.file_ref_map().get(path).cloned();
                drop(cache);
                record_read(path, &out, orig);
                return Ok((out.content, out.resolved_mode, orig, file_ref));
            }
            Prepared::Render(job) => job,
        }
    };

    let rendered = render(&job, path, crp_mode, task, tuning);

    let Some(mut cache) = write_lock_within(cache_lock, std::time::Duration::from_secs(5)) else {
        tracing::warn!("sobre_read: store-lock timeout (5s) for {path}, returning without caching");
        let out = crate::core::redaction::redact_text(&rendered);
        return Ok((out, job.resolved_mode, job.original_tokens, None));
    };
    let out = finish(
        &mut cache,
        path,
        &job.resolved_mode,
        rendered,
        crp_mode,
        task,
        tuning,
    );
    let out = settle(&mut cache, path, out);
    let orig = cache.get(path).map_or(0, |e| e.original_tokens);
    let file_ref = cache.file_ref_map().get(path).cloned();
    drop(cache);
    record_read(path, &out, orig);
    Ok((out.content, out.resolved_mode, orig, file_ref))
}

/// Resolve the `start_line`/`offset`/`limit` arguments into `(start, limit)`.
///
/// `offset` is an alias for `start_line` (1-based first line); `start_line`
/// wins if a caller passes both. `limit` (when > 0) bounds the number of lines;
/// a bare `limit` reads from line 1. Returns `None` when no windowing argument
/// is present, so the caller leaves the mode untouched.
fn resolve_line_window(
    start_line: Option<i64>,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Option<(i64, Option<i64>)> {
    let start = start_line.or(offset).map(|v| v.max(1));
    let limit = limit.filter(|&l| l > 0);
    match (start, limit) {
        (Some(s), l) => Some((s, l)),
        (None, Some(_)) => Some((1, limit)),
        (None, None) => None,
    }
}

/// Build the `lines:N-M` mode string for a resolved window. An unbounded window
/// (no `limit`) reads to EOF via the historical `999999` sentinel.
fn lines_mode(start: i64, limit: Option<i64>) -> String {
    match limit {
        Some(l) => format!("lines:{start}-{}", start + l - 1),
        None => format!("lines:{start}-999999"),
    }
}

/// Build the `anchored:N-M` mode string for a resolved window — mirrors
/// `lines_mode`, keeping the `anchored:` prefix so the render path re-attaches
/// hash anchors to the window instead of falling back to plain numbered lines.
fn anchored_lines_mode(start: i64, limit: Option<i64>) -> String {
    match limit {
        Some(l) => format!("anchored:{start}-{}", start + l - 1),
        None => format!("anchored:{start}-999999"),
    }
}

/// Apply a resolved line window to `mode`/`fresh`: the window wins over the
/// mode, so `signatures` with `start_line=10` reads `lines:10-…`.
/// `start_line=1` with no limit is a no-op so it cannot disturb an
/// auto/explicit read. An `anchored` mode is windowed in place
/// (`anchored:N-M`) instead of being collapsed to `lines:N-M`, which would
/// drop the hash anchors the caller asked for.
fn apply_line_window(
    mode: &mut String,
    fresh: &mut bool,
    _explicit_mode: bool,
    start_line: Option<i64>,
    offset: Option<i64>,
    limit: Option<i64>,
) {
    let Some((start, limit)) = resolve_line_window(start_line, offset, limit) else {
        return;
    };
    if start <= 1 && limit.is_none() {
        return;
    }
    *fresh = true;
    // Anchored gets its own windowed variant (preserves hashes for
    // sobre_patch); every other mode switches to lines:N-M to prevent
    // full-file materialization on large files.
    if mode == "anchored" {
        *mode = anchored_lines_mode(start, limit);
    } else {
        *mode = lines_mode(start, limit);
    }
}

/// Resolve the `raw=true` convenience flag into the effective explicit
/// `mode` argument. Agents reach for `raw:true` to get exact bytes; it aliases
/// to `mode="raw"` (verbatim, unframed) and wins over any caller-supplied
/// `mode`. When `raw` is unset, the caller's `mode` (if any) passes through
/// unchanged. The caller separately forces `fresh=true` for raw so a re-read
/// never collapses to an `[unchanged]`/auto-delta stub.
fn resolve_raw_alias(arg_raw: bool, mode_arg: Option<String>) -> Option<String> {
    if arg_raw {
        Some("raw".to_string())
    } else {
        mode_arg
    }
}

fn extract_file_summary(output: &str, path: &str) -> String {
    let hint = crate::core::auto_findings::extract_content_hint(output);
    if !hint.is_empty() {
        return hint;
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let line_count = output.lines().count();
    if line_count > 5 {
        format!("{ext} file, {line_count} lines")
    } else {
        String::new()
    }
}

// LOC gate: inline tests split out to keep this file under the line cap.
#[cfg(test)]
#[path = "sobre_read_inline_tests.rs"]
mod tests;

/// Read an image file and return it as MCP ContentBlock::Image for visual LLM processing.
fn read_image_file(path: &str) -> Result<ToolOutput, ErrorData> {
    use crate::core::binary_detect::{IMAGE_MAX_BYTES, image_mime_type};
    use base64::Engine;

    let metadata = std::fs::metadata(path)
        .map_err(|e| ErrorData::invalid_params(format!("Cannot read image: {e}"), None))?;

    if metadata.len() > IMAGE_MAX_BYTES {
        return Err(ErrorData::invalid_params(
            format!(
                "Image too large ({:.1} MB, limit {:.0} MB). Resize or use a smaller image.",
                metadata.len() as f64 / 1024.0 / 1024.0,
                IMAGE_MAX_BYTES as f64 / 1024.0 / 1024.0,
            ),
            None,
        ));
    }

    let mime_type = image_mime_type(path)
        .ok_or_else(|| ErrorData::invalid_params("Unsupported image format".to_string(), None))?;

    let bytes = std::fs::read(path)
        .map_err(|e| ErrorData::invalid_params(format!("Cannot read image: {e}"), None))?;

    let base64_data = base64::prelude::BASE64_STANDARD.encode(&bytes);
    let short_name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    let text_block = ContentBlock::text(format!(
        "[Image: {} ({} KB, {})]",
        short_name,
        bytes.len() / 1024,
        mime_type
    ));
    let image_block = ContentBlock::image(base64_data, mime_type);

    Ok(ToolOutput::image(vec![text_block, image_block]))
}
