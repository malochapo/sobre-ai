//! Shared batch-read implementation.
//!
//! Extracted from the deleted `ctx_multi_read` tool: `ctx_read` calls into it
//! whenever it is given a `paths` array, so removing the MCP wrapper would have
//! taken the batch path down with it. Renamed to say what it is rather than
//! which tool used to own it.

use rmcp::ErrorData;
use serde_json::{Map, Value};

use crate::server::tool_trait::{ToolContext, ToolOutput, get_bool, get_str, get_str_array};

/// Batch-read multiple files in one call. The single implementation shared by
/// the (deprecated) `ctx_multi_read` tool and by `ctx_read` when it is called
/// with a `paths` array (#509) — no duplicated batch logic across the two.
///
/// Panic guard (mirrors ctx_read): a panic in tree-sitter / compression must
/// never unwind through the dispatch `block_in_place` and kill the MCP server.
pub(crate) fn batch_read(
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<ToolOutput, ErrorData> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle_inner(args, ctx))) {
        Ok(result) => result,
        Err(_) => Err(ErrorData::internal_error(
            "ctx_multi_read panicked while processing the batch. This is a bug — please report it.",
            None,
        )),
    }
}

fn handle_inner(args: &Map<String, Value>, ctx: &ToolContext) -> Result<ToolOutput, ErrorData> {
    let raw_paths = get_str_array(args, "paths")
        .ok_or_else(|| ErrorData::invalid_params("paths array is required", None))?;

    let session_lock = ctx
        .session
        .as_ref()
        .ok_or_else(|| ErrorData::internal_error("session not available", None))?;
    let cache_lock = ctx
        .cache
        .as_ref()
        .ok_or_else(|| ErrorData::internal_error("cache not available", None))?;

    let cap = crate::core::limits::max_read_bytes() as u64;

    // Resolve + filter paths and capture the active task under one short read lock.
    // `bounded_lock` uses `Handle::block_on` directly — NOT a nested
    // `block_in_place` — because the dispatch layer already wraps this handler in
    // `block_in_place`. The previous nested `block_in_place` calls could exhaust the
    // 32-thread blocking pool under concurrent reads and freeze the server (#271).
    let (paths, current_task) = {
        let Some(session) = crate::server::bounded_lock::read(session_lock, "batch_read:session")
        else {
            return Err(ErrorData::internal_error(
                "session read-lock timeout in batch ctx_read — another tool may be holding it. Retry in a moment.",
                None,
            ));
        };
        let mut paths = Vec::with_capacity(raw_paths.len());
        for p in &raw_paths {
            let resolved = super::resolve_path_sync(&session, p)
                .map_err(|e| ErrorData::invalid_params(e, None))?;
            if crate::core::binary_detect::is_binary_file(&resolved) {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(&resolved)
                && meta.len() > cap
            {
                continue;
            }
            paths.push(resolved);
        }
        let current_task = session.task.as_ref().map(|t| t.description.clone());
        (paths, current_task)
    };

    if paths.is_empty() {
        return Err(ErrorData::invalid_params(
            "all paths are binary or exceed the size limit",
            None,
        ));
    }

    // Default to auto and let ctx_read resolve the optimal mode per file.
    // Previously this forced auto→full, which is exactly the "everything comes
    // back as full" complaint (#421): batch reads must honour auto like single
    // ctx_read does.
    let mode = get_str(args, "mode").unwrap_or_else(|| "auto".to_string());
    let fresh = get_bool(args, "fresh").unwrap_or(false);

    // Batch read under one bounded write lock. `bounded_lock` guarantees we never
    // block the runtime indefinitely and degrade gracefully on contention instead
    // of hanging; ctx_read's own fast/slow path tolerates this lock being held.
    let Some(mut cache) = crate::server::bounded_lock::write(cache_lock, "batch_read:cache") else {
        return Err(ErrorData::internal_error(
            "cache write-lock timeout in batch ctx_read — another tool may be holding it. Retry in a moment.",
            None,
        ));
    };
    let output = crate::tools::impl_multi_read::handle_with_task_fresh(
        &mut cache,
        &paths,
        &mode,
        fresh,
        ctx.crp_mode,
        current_task.as_deref(),
    );
    let mut total_original: usize = 0;
    for path in &paths {
        total_original =
            total_original.saturating_add(cache.get(path).map_or(0, |e| e.original_tokens));
    }
    let tokens = crate::core::tokens::count_tokens(&output);
    drop(cache);

    Ok(ToolOutput {
        text: output,
        original_tokens: total_original,
        saved_tokens: total_original.saturating_sub(tokens),
        mode: Some(mode),
        path: None,
        shell_outcome: None,
        content_blocks: None,
    })
}
