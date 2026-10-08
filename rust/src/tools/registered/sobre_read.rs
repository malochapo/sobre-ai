use std::sync::atomic::{AtomicBool, Ordering};
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
/// same path coordinate through a single mutex (see issue #320).
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
                    "path": { "type": "string", "description": "Absolute path" },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Batch read" },
                    "mode": {
                        "type": "string",
                        "description": "Defaults to auto (sobre picks). full=whole file, edit-ready; secret-like values show as [REDACTED:…] anchored=full with N:hh anchors to edit via sobre_patch raw=the bytes on disk: no framing, no redaction signatures=API map=structure aggressive|entropy|task=lossy summaries diff=what changed since this session last read the file reference=one line: size and type lines:N-M=window (lines:5,10-20 multi-selects) anchored:N-M=anchored window density:0.NN=target share of tokens"
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
        // #509: sobre_read absorbs multi-file batch reads (supersedes ctx_multi_read).
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
                // #1018: use try_read_owned instead of Handle::block_on to avoid
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
        let task_ref = current_task.as_deref();
        // #513: `raw=true` is the intuitive "give me the exact bytes" escape an
        // agent reaches for. Alias it to mode="raw" (verbatim, unframed) and
        // force a fresh disk read below so a re-read never collapses to an
        // `[unchanged]`/auto-delta stub. An explicit raw flag wins over `mode`.
        let arg_raw = get_bool(args, "raw").unwrap_or(false);
        let explicit_mode_arg = resolve_raw_alias(arg_raw, get_str(args, "mode"));
        let explicit_mode = explicit_mode_arg.is_some();
        // An unknown or malformed mode fails loudly (#1209: `lines:44:48` with a
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
            crate::tools::impl_smart_read::select_mode_with_task(&cache, path, task_ref)
        } else {
            tracing::debug!(
                "cache lock contested during auto-mode selection for {path}; \
                 falling back to full"
            );
            "full".to_string()
        };
        let mut fresh = get_bool(args, "fresh").unwrap_or(false);
        // #513: a raw/verbatim request always reads from disk — the whole point
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
            // SSOT mode construction via the typed `ReadMode` (#528): the typed
            // `Density` Display emits the same `density:0.NN` the pipeline parses.
            mode = crate::tools::sobre_read::ReadMode::Density(
                crate::core::aggressiveness::AggressivenessProfile::from_level(a).density_target,
            )
            .to_string();
        }
        // `start_line` (and its `offset`/`limit` aliases) can pin a line window.
        // The resolution lives in `apply_line_window`/`resolve_line_window` so
        // the runtime path and the unit tests share one implementation and can
        // never drift (GitHub #432 aliases, #259 explicit-mode, #253 line-1).
        apply_line_window(
            &mut mode,
            &mut fresh,
            explicit_mode,
            get_int(args, "start_line"),
            get_int(args, "offset"),
            get_int(args, "limit"),
        );

        let gate_result = crate::server::context_gate::pre_dispatch_read(
            path,
            &mode,
            task_ref,
            Some(&ctx.project_root),
        );
        // #513: an explicit raw/verbatim request is never silently downgraded by
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
        // the agent did not pick a mode; a mode it picked is honoured.
        let mut mode = if !explicit_mode && crate::tools::sobre_read::is_instruction_file(path) {
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
        let mut delta_explicit_note: Option<String> = None;
        if !fresh
            && explicit_mode
            && (mode == "full" || mode == "full-compact" || mode.starts_with("lines:"))
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
            let tokens = crate::core::tokens::count_tokens(&content);
            return Ok(ToolOutput {
                text: content,
                original_tokens: tokens,
                saved_tokens: 0,
                mode: Some("raw".to_string()),
                path: Some(path.to_string()),
                shell_outcome: None,
                content_blocks: None,
            });
        }

        // Compaction-aware: if host compacted since last check, reset delivery flags
        // so post-compaction reads deliver full content instead of stubs.
        if !fresh
            && let Ok(data_dir) = crate::core::data_dir::sobre_data_dir()
            && let Ok(mut cache) = cache_lock.try_write()
        {
            crate::server::compaction_sync::sync_if_compacted(&mut cache, &data_dir);
        }

        // Fast path: if both per-file lock and cache write-lock are immediately
        // available, execute inline without spawning a thread. This avoids thread +
        // channel overhead for the ~90% of calls that are cache hits.
        let read_timeout = std::time::Duration::from_secs(30);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (output, resolved_mode, original, is_cache_hit, file_ref) = {
            let crp_mode = ctx.crp_mode;
            let task_ref = current_task.as_deref();

            let fast_result = 'fast: {
                let file_lock = per_file_lock(path);
                let Some(_file_guard) = file_lock.try_lock().ok() else {
                    break 'fast None;
                };

                // Phase 1 (shared lock): the dominant case is re-reading an
                // unchanged file. Serve the `[unchanged]` stub under a *read* lock
                // so parallel reads of distinct files run concurrently instead of
                // serializing on the global write lock. `auto` is included because
                // a warm `auto` re-read of a fully-delivered file resolves to a
                // full cache-hit; `try_stub_hit_readonly` self-guards (returns None
                // unless full content was delivered and the file is unchanged), so a
                // first or compressed-only `auto` read still falls through to
                // Phase 2. When aggressiveness is set `mode` was already rewritten
                // to `density:` upstream, so it never reaches this `auto` branch.
                if !fresh
                    && (mode == "full" || mode == "full-compact" || mode == "auto")
                    && let Ok(cache) = cache_lock.try_read()
                    && let Some(read_output) =
                        crate::tools::sobre_read::try_stub_hit_readonly(&cache, path)
                {
                    let hit = read_output.is_cache_hit;
                    let content = read_output.content;
                    let rmode = read_output.resolved_mode;
                    let orig = cache.get(path).map_or(0, |e| e.original_tokens);
                    let fref = cache.file_ref_map().get(path).cloned();
                    break 'fast Some((content, rmode, orig, hit, fref));
                }

                // Phase 2 (write lock): cache miss, changed file, or non-stub
                // modes (map/signatures/diff/lines) that mutate cache state.
                let Some(mut cache) = cache_lock.try_write().ok() else {
                    break 'fast None;
                };
                let read_output = if fresh {
                    crate::tools::sobre_read::handle_fresh_with_task_resolved_tuned(
                        &mut cache,
                        path,
                        &mode,
                        crp_mode,
                        task_ref,
                        aggressiveness,
                        &protect,
                    )
                } else {
                    crate::tools::sobre_read::handle_with_task_resolved_tuned(
                        &mut cache,
                        path,
                        &mode,
                        crp_mode,
                        task_ref,
                        aggressiveness,
                        &protect,
                    )
                };
                let hit = read_output.is_cache_hit;
                let content = read_output.content;
                let rmode = read_output.resolved_mode;
                let orig = cache.get(path).map_or(0, |e| e.original_tokens);
                let fref = cache.file_ref_map().get(path).cloned();
                Some((content, rmode, orig, hit, fref))
            };

            if let Some(result) = fast_result {
                result
            } else {
                let cache_lock = cache_lock.clone();
                let mode = mode.clone();
                let task_owned = current_task.clone();
                let protect_owned = protect.clone();
                let path_owned = path.to_string();
                let cancel_flag = cancelled.clone();
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                std::thread::spawn(move || {
                    let file_lock = per_file_lock(&path_owned);

                    let _file_guard = {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(25);
                        loop {
                            if cancel_flag.load(Ordering::Relaxed) {
                                return;
                            }
                            if let Ok(guard) = file_lock.try_lock() {
                                break guard;
                            }
                            if std::time::Instant::now() >= deadline {
                                tracing::error!(
                                    "sobre_read: per-file lock timeout after 25s for {path_owned}"
                                );
                                let _ = tx.send((
                                    format!("per-file lock contention for {path_owned} — retry in a moment"),
                                    "error".to_string(), 0, false, None,
                                ));
                                return;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    };

                    if cancel_flag.load(Ordering::Relaxed) {
                        return;
                    }

                    // ── Two-Phase Read (#1098) ──────────────────────────
                    //
                    // Phase 1 (read lock): try the [unchanged] stub — this is the
                    // ~70% case (repeated reads of unchanged files). Previously
                    // missing in the slow path, forcing every slow-path call into
                    // the expensive write-lock branch.
                    if !fresh
                        && (mode == "full" || mode == "full-compact" || mode == "auto")
                        && let Ok(cache) = cache_lock.try_read()
                        && let Some(read_output) =
                            crate::tools::sobre_read::try_stub_hit_readonly(&cache, &path_owned)
                    {
                        let content = read_output.content;
                        let rmode = read_output.resolved_mode;
                        let orig = cache.get(&path_owned).map_or(0, |e| e.original_tokens);
                        let hit = true;
                        let fref = cache.file_ref_map().get(path_owned.as_str()).cloned();
                        let _ = tx.send((content, rmode, orig, hit, fref));
                        return;
                    }

                    // Phase 2a: disk I/O under per-file lock but WITHOUT cache lock.
                    let preread = crate::tools::sobre_read::read_file_lossy(&path_owned).ok();

                    if cancel_flag.load(Ordering::Relaxed) {
                        return;
                    }

                    // ── Phase 2b: Three-sub-phase read (#807) ──────────
                    //
                    // Previously held the global cache write-lock for the
                    // entire computation (tree-sitter, entropy compression).
                    // For large files this caused 30+ second lock holds and
                    // cascading timeouts for all concurrent tool calls.
                    //
                    // New: prepare (brief lock) → compute (no lock) → store (brief lock).

                    let task_ref = task_owned.as_deref();
                    let tuning = crate::tools::sobre_read::ReadTuning::resolve(
                        aggressiveness,
                        &protect_owned,
                    );

                    // Helper: acquire write lock with deadline.
                    macro_rules! acquire_write {
                        ($deadline_secs:expr, $label:expr) => {{
                            let deadline = std::time::Instant::now()
                                + std::time::Duration::from_secs($deadline_secs);
                            loop {
                                if cancel_flag.load(Ordering::Relaxed) {
                                    return;
                                }
                                if let Ok(guard) = cache_lock.try_write() {
                                    break guard;
                                }
                                if std::time::Instant::now() >= deadline {
                                    tracing::error!(
                                        "sobre_read: cache write-lock timeout ({}) for {path_owned}",
                                        $label,
                                    );
                                    let _ = tx.send((
                                        format!(
                                            "cache lock contention for {path_owned} — retry in a moment"
                                        ),
                                        "error".into(),
                                        0,
                                        false,
                                        None,
                                    ));
                                    return;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(50));
                            }
                        }};
                    }

                    // 2b-i: Brief write lock — prepare cache state, resolve
                    // mode, check for hits. Sub-millisecond: HashMap lookups,
                    // staleness checks, raw-content storage for new files.
                    #[allow(clippy::large_enum_variant)]
                    enum PrepareOutcome {
                        Hit(String, String, usize, bool, Option<String>),
                        Compute {
                            resolved_mode: String,
                            content: String,
                            original_tokens: usize,
                        },
                    }

                    // The content to render when the cache holds none: the
                    // preread, else the file itself. `None` after an error
                    // was sent: a file that cannot be read is never empty.
                    let content_or_disk = |cached: Option<String>| -> Option<String> {
                        if let Some(c) = cached.or_else(|| preread.clone()) {
                            return Some(c);
                        }
                        match crate::tools::sobre_read::read_file_lossy(&path_owned) {
                            Ok(raw) => Some(raw),
                            Err(e) => {
                                let _ = tx.send((
                                    format!("{path_owned}: {e}"),
                                    "error".into(),
                                    0,
                                    false,
                                    None,
                                ));
                                None
                            }
                        }
                    };

                    let outcome = {
                        let mut cache = acquire_write!(10, "prepare 10s");

                        if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
                            bt.next_seq();
                        }

                        let mode_eff = mode.clone();

                        if fresh {
                            cache.invalidate(&path_owned);
                        }

                        if !fresh {
                            let stale = cache.get(&path_owned).is_some_and(|e| {
                                crate::core::cache::is_cache_entry_stale_verified(
                                    &path_owned,
                                    e.stored_mtime,
                                    &e.hash,
                                )
                            });
                            if stale {
                                cache.invalidate(&path_owned);
                            }
                        }

                        let snap = cache
                            .get(&path_owned)
                            .map(|e| (e.original_tokens, e.content()));

                        if let Some((orig_tok, content_opt)) = snap {
                            let resolved = if mode_eff == "auto" {
                                tuning.auto_density_mode().unwrap_or_else(|| {
                                    crate::tools::sobre_read::resolve_auto_mode(
                                        Some(&cache),
                                        &path_owned,
                                        orig_tok,
                                        task_ref,
                                    )
                                })
                            } else {
                                mode_eff
                            };

                            if (resolved == "full" || resolved == "full-compact")
                                && let Some(out) = crate::tools::sobre_read::try_stub_hit_readonly(
                                    &cache,
                                    &path_owned,
                                )
                            {
                                let orig = cache.get(&path_owned).map_or(0, |e| e.original_tokens);
                                let fref = cache.file_ref_map().get(path_owned.as_str()).cloned();
                                PrepareOutcome::Hit(
                                    out.content,
                                    out.resolved_mode,
                                    orig,
                                    true,
                                    fref,
                                )
                            } else if crate::tools::sobre_read::is_cacheable_mode(&resolved) {
                                let ck = crate::tools::sobre_read::compressed_cache_key(
                                    &resolved,
                                    crp_mode,
                                    task_ref,
                                    tuning.aggressiveness,
                                    tuning.protect,
                                );
                                if let Some(hit) = cache.get_compressed(&path_owned, &ck).cloned() {
                                    let hit = crate::core::redaction::redact_text(&hit);
                                    let orig =
                                        cache.get(&path_owned).map_or(0, |e| e.original_tokens);
                                    let fref =
                                        cache.file_ref_map().get(path_owned.as_str()).cloned();
                                    PrepareOutcome::Hit(hit, resolved, orig, true, fref)
                                } else {
                                    let Some(c) = content_or_disk(content_opt) else {
                                        return;
                                    };
                                    PrepareOutcome::Compute {
                                        resolved_mode: resolved,
                                        content: c,
                                        original_tokens: orig_tok,
                                    }
                                }
                            } else {
                                let Some(c) = content_or_disk(content_opt) else {
                                    return;
                                };
                                PrepareOutcome::Compute {
                                    resolved_mode: resolved,
                                    content: c,
                                    original_tokens: orig_tok,
                                }
                            }
                        } else {
                            // A file that cannot be read is an error, as on
                            // the fast path: never an empty file.
                            let Some(raw) = content_or_disk(None) else {
                                return;
                            };
                            let sr = cache.store(&path_owned, &raw);
                            let resolved = if mode_eff == "auto" {
                                tuning.auto_density_mode().unwrap_or_else(|| {
                                    crate::tools::sobre_read::resolve_auto_mode(
                                        None,
                                        &path_owned,
                                        sr.original_tokens,
                                        task_ref,
                                    )
                                })
                            } else {
                                mode_eff
                            };
                            PrepareOutcome::Compute {
                                resolved_mode: resolved,
                                content: raw,
                                original_tokens: sr.original_tokens,
                            }
                        }
                    }; // write lock released

                    if let PrepareOutcome::Hit(c, rm, orig, hit, fref) = outcome {
                        let _ = tx.send((c, rm, orig, hit, fref));
                        return;
                    }
                    let PrepareOutcome::Compute {
                        resolved_mode,
                        content: compute_content,
                        original_tokens,
                    } = outcome
                    else {
                        unreachable!()
                    };

                    if cancel_flag.load(Ordering::Relaxed) {
                        return;
                    }

                    // 2b-ii: Heavy computation WITHOUT cache lock.
                    // Tree-sitter, entropy compression, mode rendering all
                    // run under the per-file mutex only (serializes same-file
                    // reads, but does not block other files or tool calls).
                    let short = crate::core::protocol::shorten_path(&path_owned);
                    let ext_s = std::path::Path::new(&*path_owned)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");

                    let (mut computed, rmode) = if resolved_mode == "full"
                        || resolved_mode == "full-compact"
                    {
                        if resolved_mode == "full-compact" {
                            let (out, _) = crate::tools::sobre_read::format_full_compact_output(
                                &compute_content,
                            );
                            (out, "full-compact".to_string())
                        } else {
                            let lc = compute_content.lines().count();
                            let (out, _) = crate::tools::sobre_read::format_full_output(
                                &short,
                                ext_s,
                                &compute_content,
                                lc,
                                task_ref,
                            );
                            let ft = crate::core::tokens::count_tokens(&out);
                            let out = crate::tools::sobre_read::cap_to_raw(
                                out,
                                ft,
                                &compute_content,
                                original_tokens,
                                "full",
                            );
                            (out, "full".to_string())
                        }
                    } else {
                        let (out, _) = crate::tools::sobre_read::process_mode_tuned(
                            &compute_content,
                            &resolved_mode,
                            &short,
                            ext_s,
                            original_tokens,
                            crp_mode,
                            &path_owned,
                            task_ref,
                            tuning,
                        );
                        let out = if crate::tools::sobre_read::mode_allows_raw_cap(&resolved_mode) {
                            let ft = crate::core::tokens::count_tokens(&out);
                            crate::tools::sobre_read::cap_to_raw(
                                out,
                                ft,
                                &compute_content,
                                original_tokens,
                                &resolved_mode,
                            )
                        } else {
                            out
                        };
                        (out, resolved_mode)
                    };

                    computed = crate::core::redaction::redact_text(&computed);

                    if cancel_flag.load(Ordering::Relaxed) {
                        return;
                    }

                    // 2b-iii: Brief write lock — store result + metadata.
                    // Sub-millisecond: HashMap insert + stats snapshot.
                    // Graceful degradation: if the lock cannot be acquired
                    // within 5s, return the result without caching it.
                    {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(5);
                        let cache_guard = loop {
                            if cancel_flag.load(Ordering::Relaxed) {
                                return;
                            }
                            if let Ok(g) = cache_lock.try_write() {
                                break Some(g);
                            }
                            if std::time::Instant::now() >= deadline {
                                tracing::warn!(
                                    "sobre_read: store-lock timeout (5s) for {path_owned},                                      returning without caching"
                                );
                                break None;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        };

                        if let Some(mut cache) = cache_guard {
                            if crate::tools::sobre_read::is_cacheable_mode(&rmode) {
                                let ck = crate::tools::sobre_read::compressed_cache_key(
                                    &rmode,
                                    crp_mode,
                                    task_ref,
                                    tuning.aggressiveness,
                                    tuning.protect,
                                );
                                cache.set_compressed(&path_owned, &ck, computed.clone());
                            }
                            if rmode == "full" || rmode == "full-compact" {
                                cache.mark_full_delivered(&path_owned);
                            }
                            if let Some(entry) = cache.get_mut(&path_owned) {
                                entry.last_mode.clone_from(&rmode);
                            }
                            if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
                                bt.record_read(
                                    &path_owned,
                                    &rmode,
                                    crate::core::tokens::count_tokens(&computed),
                                    original_tokens,
                                );
                            }
                            let orig = cache.get(&path_owned).map_or(0, |e| e.original_tokens);
                            let fref = cache.file_ref_map().get(path_owned.as_str()).cloned();
                            let _ = tx.send((computed, rmode, orig, false, fref));
                        } else {
                            let _ = tx.send((computed, rmode, original_tokens, false, None));
                        }
                    }
                });
                if let Ok(result) = rx.recv_timeout(read_timeout) {
                    result
                } else {
                    cancelled.store(true, Ordering::Relaxed);
                    tracing::error!("sobre_read timed out after {read_timeout:?} for {path}");
                    let msg = format!(
                        "ERROR: sobre_read timed out after {}s reading {path}. \
                     The file may be very large or a blocking I/O issue occurred. \
                     Try mode=\"lines:1-100\" for a partial read.",
                        read_timeout.as_secs()
                    );
                    return Err(ErrorData::internal_error(msg, None));
                }
            } // end else (slow path)
        };

        if resolved_mode == "error" {
            return Err(ErrorData::invalid_params(output, None));
        }

        let output_tokens = crate::core::tokens::count_tokens(&output);
        let saved = original.saturating_sub(output_tokens);

        // Session updates (bounded lock — 10s timeout, read already succeeded)
        let mut traversal_working_set: Vec<String> = Vec::new();
        let project_root_snapshot;
        {
            let rt = tokio::runtime::Handle::current();
            let session_guard = rt.block_on(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                session_lock.write(),
            ));
            if let Ok(mut session) = session_guard {
                session.touch_file(path, file_ref.as_deref(), &resolved_mode, original);
                // Capture the recent working set (under the lock) so the
                // background thread can record a traversal/co-access edge (#289).
                traversal_working_set =
                    crate::core::tool_lifecycle::recent_working_set(&session, path);
                let file_summary = extract_file_summary(&output, path);
                if !file_summary.is_empty() {
                    session.set_file_summary(path, &file_summary);
                }
                if is_cache_hit {
                    session.record_cache_hit();
                }
                let root_missing = session
                    .project_root
                    .as_deref()
                    .is_none_or(|r| r.trim().is_empty());
                if root_missing && let Some(root) = crate::core::protocol::detect_project_root(path)
                {
                    session.project_root = Some(root);
                }
                project_root_snapshot = session
                    .project_root
                    .clone()
                    .unwrap_or_else(|| ".".to_string());
            } else {
                tracing::warn!(
                    "session write-lock timeout (5s) in sobre_read post-update for {path}"
                );
                project_root_snapshot = ctx.project_root.clone();
            }
        }

        // The co-access edge is a side effect that never influences this
        // response; keep its disk I/O off the hot path (#149).
        {
            let path_bg = path.to_string();
            let project_root_bg = project_root_snapshot.clone();
            std::thread::spawn(move || {
                // A panic here must not poison locks or leave a zombie thread;
                // it never affects the already-returned read response.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    // Traversal/co-access edge: this read fired together with the
                    // recent working set captured under the session lock (#289).
                    if let Some(root) =
                        crate::core::tool_lifecycle::usable_root(Some(project_root_bg.as_str()))
                    {
                        crate::core::cooccurrence::record_focus_access(
                            root,
                            &path_bg,
                            &traversal_working_set,
                        );
                    }
                }));
            });
        }

        // Cross-source hints: if the property graph has cross-source edges
        // pointing to this file, append compact hints so the agent knows about
        // related issues/PRs/schemas without a separate tool call (#682). Only
        // touch the DB when it already exists — never create graph.db on a read.
        let hints_suffix = {
            let graph_db =
                crate::core::property_graph::graph_dir(&ctx.project_root).join("graph.db");
            let edges = if graph_db.exists() {
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
        // #977: notices (mode override, budget, degradation, delta) go BEFORE the
        // payload so client-side truncation of large outputs cannot hide them.
        let final_output = if !warnings.is_empty() {
            format!("{}\n\n{output}{hints_suffix}", warnings.join("\n"))
        } else if hints_suffix.is_empty() {
            output
        } else {
            format!("{output}{hints_suffix}")
        };
        let proactive_query = format!(
            "sobre_read path={path} mode={resolved_mode} task={}",
            task_ref.unwrap_or_default()
        );
        let final_output = if let Some(block) =
            crate::core::relevance_tracker::proactive_context_for_path(&proactive_query, path)
        {
            format!("{final_output}{block}")
        } else {
            final_output
        };

        Ok(ToolOutput {
            text: final_output,
            original_tokens: original,
            saved_tokens: saved,
            mode: Some(resolved_mode),
            path: Some(path.to_string()),
            shell_outcome: None,
            content_blocks: None,
        })
    }
}

/// Resolve the `start_line`/`offset`/`limit` arguments into `(start, limit)`.
///
/// `offset` is an alias for `start_line` (1-based first line); `start_line`
/// wins if a caller passes both. `limit` (when > 0) bounds the number of lines;
/// a bare `limit` reads from line 1. Returns `None` when no windowing argument
/// is present, so the caller leaves the mode untouched (GitHub #432).
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

/// Build the `anchored:N-M` mode string for a resolved window (#811) — mirrors
/// `lines_mode`, keeping the `anchored:` prefix so the render path re-attaches
/// hash anchors to the window instead of falling back to plain numbered lines.
fn anchored_lines_mode(start: i64, limit: Option<i64>) -> String {
    match limit {
        Some(l) => format!("anchored:{start}-{}", start + l - 1),
        None => format!("anchored:{start}-999999"),
    }
}

/// Apply a resolved line window to `mode`/`fresh`. An explicit non-lines mode
/// (map/signatures/…) is never clobbered (#259), and `start_line=1` with no
/// limit is a no-op so it cannot disturb an auto/explicit read (#253). An
/// explicit `anchored` mode is windowed in place (`anchored:N-M`, #811)
/// instead of being collapsed to `lines:N-M` — that would silently drop the
/// hash anchors the caller asked for, and previously let a bounded anchored
/// read fall through to rendering (and erroring on) the whole file.
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
    // #811: anchored gets its own windowed variant (preserves hashes for
    // sobre_patch); every other mode switches to lines:N-M to prevent
    // full-file materialization on large files.
    if mode == "anchored" {
        *mode = anchored_lines_mode(start, limit);
    } else {
        *mode = lines_mode(start, limit);
    }
}

/// #513: resolve the `raw=true` convenience flag into the effective explicit
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

// #660 LOC gate: inline tests split out to keep this file under the line cap.
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

    Ok(ToolOutput::image(
        vec![text_block, image_block],
        path.to_string(),
    ))
}
