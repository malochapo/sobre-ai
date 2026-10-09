use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{
    McpTool, ToolContext, ToolOutput, get_bool, get_str, require_resolved_path,
};
use crate::tool_defs::tool_def;
use crate::tools::impl_edit::CacheEffect;

pub struct SobrePatchTool;

impl McpTool for SobrePatchTool {
    fn name(&self) -> &'static str {
        "sobre_patch"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_patch",
            "Safe file edit. Anchored ops use line+hash from sobre_read(mode=\"anchored\"); \
             CONFLICT means those lines changed: the current ones come back with fresh anchors. \
             replace_unique(path,old_text,new_text) is a no-read, exact unique replacement. \
             replace_symbol/create/replace_all supported. ops[] runs in order, across files: \
             one file's anchored ops apply all-or-nothing, and the batch stops at the first \
             failure, saying what was not applied. A syntax error an edit introduces is \
             reported, not refused. dry_run=true shows the diff and writes nothing.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "op": { "type": "string", "enum": ["set_line", "replace_lines", "insert_after", "delete", "replace_unique", "replace_symbol", "create", "replace_all"] },
                    "line": { "type": "integer" },
                    "hash": { "type": "string" },
                    "start_line": { "type": "integer" },
                    "start_hash": { "type": "string" },
                    "end_line": { "type": "integer" },
                    "end_hash": { "type": "string" },
                    "new_text": { "type": "string" },
                    "old_text": { "type": "string" },
                    "name": { "type": "string" },
                    "find": { "type": "string" },
                    "replace": { "type": "string" },
                    "dry_run": { "type": "boolean" },
                    "ops": { "type": "array", "items": { "type": "object" } }
                },
                // Per-op required params encoded as the source of truth:
                // a client reading the schema knows which fields an op needs
                // BEFORE calling. Each `if` requires `op` so it stays dormant for
                // batch calls (which carry `op` inside ops[], not at top level).
                // Only ops with UNCONDITIONAL requirements are listed — insert_after
                // (hash optional when line=0) and delete (single vs range) keep their
                // requirements in the parser to avoid rejecting valid calls.
                "allOf": [
                    { "if": { "properties": { "op": { "const": "set_line" } }, "required": ["op"] },
                      "then": { "required": ["op", "line", "hash", "new_text"] } },
                    { "if": { "properties": { "op": { "const": "replace_lines" } }, "required": ["op"] },
                      "then": { "required": ["op", "start_line", "end_line", "new_text"] } },
                    { "if": { "properties": { "op": { "const": "replace_unique" } }, "required": ["op"] },
                      "then": { "required": ["op", "old_text", "new_text"] } },
                    { "if": { "properties": { "op": { "const": "replace_symbol" } }, "required": ["op"] },
                      "then": { "required": ["op", "new_text"] } },
                    { "if": { "properties": { "op": { "const": "create" } }, "required": ["op"] },
                      "then": { "required": ["op", "new_text"] } },
                    { "if": { "properties": { "op": { "const": "replace_all" } }, "required": ["op"] },
                      "then": { "required": ["op", "find", "replace"] } }
                ]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        // replace_symbol is a whole-symbol rewrite — delegate to the LSP/IDE-aware
        // sobre_refactor so there is one symbol-edit implementation.
        if crate::tools::sobre_patch::is_replace_symbol(args) {
            return delegate_replace_symbol(args, ctx);
        }

        if get_str(args, "op").as_deref() == Some("replace_unique") {
            return delegate_replace_unique(args, ctx);
        }

        // Replace_all short-circuits before anchor parsing.
        if get_str(args, "op").as_deref() == Some("replace_all") {
            return handle_replace_all(args, ctx);
        }

        // Replace_unique/replace_symbol are allowed inside ops[]. Split
        // the batch into runs: each delegated op reuses its single-op write
        // path; consecutive anchored ops keep shared-preimage batch semantics.
        if let Some(arr) = args.get("ops").and_then(Value::as_array)
            && arr.iter().any(|v| delegated_op_kind(v).is_some())
        {
            return handle_mixed_batch(args, arr, ctx);
        }

        handle_anchored(args, ctx)
    }
}

/// Whether an edit's own result reports a failure (a success starts with `✓`
/// or `DRY RUN`).
fn is_failure(text: &str) -> bool {
    text.starts_with("ERROR") || text.starts_with("CONFLICT")
}

/// The line that ends a batch at its first failure.
fn stopped(not_applied: usize) -> String {
    format!("ERROR: stopped here; {not_applied} later op(s) or file(s) were not applied")
}

/// Anchored pipeline: a single anchored op or an ops[] batch of anchored edits
/// (grouped per file, all-or-nothing per file). Split out of `handle` so the
/// mixed-batch path can reuse it per anchored run.
fn handle_anchored(args: &Map<String, Value>, ctx: &ToolContext) -> Result<ToolOutput, ErrorData> {
    Ok(ToolOutput::simple(run_anchored(args, ctx)?.0))
}

/// [`handle_anchored`]'s text, and whether it stopped at a failure.
fn run_anchored(args: &Map<String, Value>, ctx: &ToolContext) -> Result<(String, bool), ErrorData> {
    let dry_run = get_bool(args, "dry_run").unwrap_or(false);

    // A batch (`ops[]`) groups its ops by each op's own `path`, so a
    // batch may span files and needs no top-level `path`. A single op still
    // requires the top-level `path`.
    let groups = plan_groups(args, ctx)?;
    let total = groups.len();

    let mut texts = Vec::with_capacity(total);
    for (i, (path, ops)) in groups.into_iter().enumerate() {
        let patch_params = crate::tools::sobre_patch::PatchParams {
            path: path.clone(),
            ops,
            dry_run,
        };
        let output = apply_one(ctx, &patch_params)?;
        let failed = is_failure(&output);
        texts.push(format!("[{path}]\n{output}"));
        // Each file's ops apply all-or-nothing; the files after a failed one
        // are left alone, and the result says so.
        if failed {
            if i + 1 < total {
                texts.push(stopped(total - i - 1));
            }
            return Ok((texts.join("\n\n"), true));
        }
    }

    Ok((texts.join("\n\n"), false))
}

/// `Some(op)` when a batch op must be dispatched through its own single-op
/// write path rather than the anchored pipeline.
fn delegated_op_kind(v: &Value) -> Option<&str> {
    v.get("op")
        .and_then(Value::as_str)
        .filter(|k| matches!(*k, "replace_unique" | "replace_symbol"))
}

/// An ops[] batch containing replace_unique/replace_symbol. Ops apply
/// strictly in the order given, as if issued as separate calls: consecutive
/// anchored ops flush as one shared-preimage batch; each delegated op is
/// dispatched through the same delegate as its top-level form, evaluated
/// against the file state left by the preceding ops. The batch stops at the
/// first failure and says how many ops were not applied.
fn handle_mixed_batch(
    args: &Map<String, Value>,
    arr: &[Value],
    ctx: &ToolContext,
) -> Result<ToolOutput, ErrorData> {
    let mut texts: Vec<String> = Vec::new();
    let mut run: Vec<Value> = Vec::new();
    for (i, v) in arr.iter().enumerate() {
        let obj = v.as_object().ok_or_else(|| {
            ErrorData::invalid_params(format!("ops[{i}] must be an object"), None)
        })?;
        let Some(kind) = delegated_op_kind(v) else {
            run.push(v.clone());
            continue;
        };
        if flush_anchored_run(args, ctx, &mut run, &mut texts)? {
            texts.push(stopped(arr.len() - i));
            return Ok(ToolOutput::simple(texts.join("\n\n")));
        }

        let (sub_args, sub_ctx) = delegated_op_call(args, obj, ctx, i, kind)?;
        let out = if kind == "replace_unique" {
            delegate_replace_unique(&sub_args, &sub_ctx)
        } else {
            delegate_replace_symbol(&sub_args, &sub_ctx)
        }
        .map_err(|e| {
            let applied = if texts.is_empty() {
                ""
            } else {
                " (earlier ops in this batch were already applied)"
            };
            ErrorData::invalid_params(format!("ops[{i}] ({kind}): {}{applied}", e.message), None)
        })?;
        let label = get_str(&sub_args, "path").unwrap_or_else(|| kind.to_string());
        let failed = is_failure(&out.text);
        texts.push(format!("[{label}]\n{}", out.text));
        if failed {
            if i + 1 < arr.len() {
                texts.push(stopped(arr.len() - i - 1));
            }
            return Ok(ToolOutput::simple(texts.join("\n\n")));
        }
    }
    flush_anchored_run(args, ctx, &mut run, &mut texts)?;

    Ok(ToolOutput::simple(texts.join("\n\n")))
}

/// Flush buffered anchored ops through the anchored pipeline as one batch;
/// true when it stopped at a failure.
fn flush_anchored_run(
    args: &Map<String, Value>,
    ctx: &ToolContext,
    run: &mut Vec<Value>,
    texts: &mut Vec<String>,
) -> Result<bool, ErrorData> {
    if run.is_empty() {
        return Ok(false);
    }
    let mut sub = args.clone();
    sub.insert("ops".into(), Value::Array(std::mem::take(run)));
    let (text, failed) = run_anchored(&sub, ctx)?;
    texts.push(text);
    Ok(failed)
}

/// Build the (args, ctx) for one delegated batch op: the op object inherits the
/// batch's top-level `path`/`dry_run` when it doesn't set its own, and the op's
/// path is resolved into a per-op ctx — the dispatch layer only pre-resolves
/// the top-level `path`, which may differ or be absent.
fn delegated_op_call(
    args: &Map<String, Value>,
    op: &Map<String, Value>,
    ctx: &ToolContext,
    i: usize,
    kind: &str,
) -> Result<(Map<String, Value>, ToolContext), ErrorData> {
    let mut sub = op.clone();
    for key in ["path", "dry_run"] {
        if !sub.contains_key(key)
            && let Some(v) = args.get(key)
        {
            sub.insert(key.to_string(), v.clone());
        }
    }

    let mut sub_ctx = ctx.clone();
    sub_ctx.resolved_paths.remove("path");
    sub_ctx.path_errors.remove("path");
    match get_str(&sub, "path") {
        Some(raw) => {
            let resolved = sub_ctx
                .resolve_path_sync(&raw)
                .map_err(|e| ErrorData::invalid_params(format!("ops[{i}]: path: {e}"), None))?;
            sub_ctx.resolved_paths.insert("path".to_string(), resolved);
        }
        // replace_symbol's `name` route resolves its own path inside
        // sobre_refactor; replace_unique always needs one.
        None if kind == "replace_unique" => {
            return Err(ErrorData::invalid_params(
                format!("ops[{i}] needs its own 'path' (no top-level 'path' to fall back to)"),
                None,
            ));
        }
        None => {}
    }
    Ok((sub, sub_ctx))
}

/// One-shot content-anchored edit: exactly one match of `old_text` becomes
/// `new_text`.
fn delegate_replace_unique(
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<ToolOutput, ErrorData> {
    let (old_text, new_text) =
        unique_texts(args).map_err(|message| ErrorData::invalid_params(message, None))?;
    replace_text(args, ctx, old_text, new_text, false)
}

fn unique_texts(args: &Map<String, Value>) -> Result<(String, String), String> {
    let old_text = get_str(args, "old_text")
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "replace_unique requires non-empty old_text".to_string())?;
    let new_text =
        get_str(args, "new_text").ok_or_else(|| "replace_unique requires new_text".to_string())?;
    Ok((old_text, new_text))
}

/// `replace_unique` and `replace_all` share one write path: per-file lock,
/// preimage guard, permission-preserving atomic write, session and cache
/// bookkeeping.
fn replace_text(
    args: &Map<String, Value>,
    ctx: &ToolContext,
    old_text: String,
    new_text: String,
    replace_all: bool,
) -> Result<ToolOutput, ErrorData> {
    let params = crate::tools::impl_edit::EditParams {
        path: require_resolved_path(ctx, args, "path")?,
        old_text,
        new_text,
        replace_all,
        dry_run: get_bool(args, "dry_run").unwrap_or(false),
    };
    let text = locked_edit(ctx, &params.path, |last_mode| {
        let (output, effect) = crate::tools::impl_edit::run_io(&params, last_mode);
        // Quality loop: feed success/old_text-miss back into per-(ext × mode)
        // stats and the one-shot read escalation.
        crate::tools::impl_edit::record_outcome(&params, last_mode, &output, &effect);
        (output, effect)
    })?;
    Ok(ToolOutput::simple(text))
}

/// Build the per-file work list. A single op targets the top-level `path`
/// (still required). A batch (`ops[]`) groups its ops by each op's own `path`,
/// falling back to the top-level `path`, so a batch may span files without a
/// top-level `path`.
fn plan_groups(
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<Vec<(String, Vec<crate::tools::sobre_patch::AnchorOp>)>, ErrorData> {
    let Some(ops_val) = args.get("ops") else {
        let path = require_resolved_path(ctx, args, "path")?;
        let ops = crate::tools::sobre_patch::parse_ops(args)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        return Ok(vec![(path, ops)]);
    };

    let arr = ops_val
        .as_array()
        .ok_or_else(|| ErrorData::invalid_params("ops must be an array of edit objects", None))?;
    let grouped = group_ops_by_path(arr, get_str(args, "path").as_deref())
        .map_err(|e| ErrorData::invalid_params(e, None))?;

    let mut groups = Vec::with_capacity(grouped.len());
    for (raw_path, op_objs) in grouped {
        let resolved = ctx
            .resolve_path_sync(&raw_path)
            .map_err(|e| ErrorData::invalid_params(format!("path: {e}"), None))?;
        let sub = Map::from_iter([("ops".to_string(), Value::Array(op_objs))]);
        let ops = crate::tools::sobre_patch::parse_ops(&sub)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        groups.push((resolved, ops));
    }
    Ok(groups)
}

/// Pure grouping: bucket batch op objects by their own `path` (or the batch's
/// top-level `path` fallback), preserving first-seen file order. Errors if an op
/// names no path and there is no top-level fallback.
fn group_ops_by_path(
    ops: &[Value],
    top_path: Option<&str>,
) -> Result<Vec<(String, Vec<Value>)>, String> {
    if ops.is_empty() {
        return Err("ops[] is empty — provide at least one edit".to_string());
    }
    let mut order: Vec<String> = Vec::new();
    let mut by_path: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        let obj = op
            .as_object()
            .ok_or_else(|| format!("ops[{i}] must be an object"))?;
        let raw = obj
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| top_path.map(str::to_string))
            .ok_or_else(|| {
                format!("ops[{i}] needs its own 'path' (no top-level 'path' to fall back to)")
            })?;
        if !by_path.contains_key(&raw) {
            order.push(raw.clone());
        }
        by_path.entry(raw).or_default().push(op.clone());
    }
    Ok(order
        .into_iter()
        .map(|p| {
            let ops = by_path.remove(&p).unwrap_or_default();
            (p, ops)
        })
        .collect())
}

/// Apply one file's anchored patch through [`locked_edit`]. Returns the
/// rendered patch/CONFLICT text. Shared by the single-op and the per-file
/// batch paths.
fn apply_one(
    ctx: &ToolContext,
    params: &crate::tools::sobre_patch::PatchParams,
) -> Result<String, ErrorData> {
    locked_edit(ctx, &params.path, |last_mode| {
        let (output, effect) = crate::tools::sobre_patch::run_io(params, last_mode);
        crate::tools::sobre_patch::record_outcome(params, last_mode, &output, &effect);
        (output, effect)
    })
}

/// Runs one file edit: takes the per-file lock, reads the file's last read
/// mode, runs `edit` (the disk I/O) without the global cache lock, then
/// applies its cache effect and marks the file modified in the session.
fn locked_edit(
    ctx: &ToolContext,
    path: &str,
    edit: impl FnOnce(&str) -> (String, CacheEffect),
) -> Result<String, ErrorData> {
    tokio::task::block_in_place(|| {
        let cache_lock = ctx
            .cache
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("cache not available", None))?;
        let rt = tokio::runtime::Handle::current();

        // Serialize edits to the SAME file via the shared per-file lock (the same
        // registry sobre_read uses), so anchored and text edits of one file
        // never interleave. Correctness across processes still rests on the
        // TOCTOU preimage guard + atomic rename.
        let file_lock = crate::core::path_locks::per_file_lock(path);
        let _file_guard = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                if let Ok(guard) = file_lock.try_lock() {
                    break guard;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(ErrorData::internal_error(
                        format!(
                            "per-file edit lock contention for {path} — another edit to the same file is in progress, retry in a moment"
                        ),
                        None,
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };

        // Brief shared lock: the recorded read mode, for auto-escalation and
        // the quality loop. On contention, skip it rather than block the I/O.
        let last_mode = match rt.block_on(tokio::time::timeout(
            std::time::Duration::from_secs(5),
            cache_lock.read(),
        )) {
            Ok(cache) => cache
                .get(path)
                .map(|e| e.last_mode.clone())
                .unwrap_or_default(),
            Err(_) => String::new(),
        };

        // Heavy disk I/O — no global cache lock held here.
        let (output, effect) = edit(&last_mode);
        let wrote = matches!(effect, CacheEffect::Invalidate);

        if !matches!(effect, CacheEffect::None) {
            match rt.block_on(tokio::time::timeout(
                std::time::Duration::from_secs(5),
                cache_lock.write(),
            )) {
                Ok(mut cache) => {
                    crate::tools::impl_edit::apply_cache_effect(&mut cache, path, effect);
                }
                Err(_) => {
                    tracing::warn!(
                        "sobre_patch: cache write-lock timeout (5s) applying post-edit cache effect for {path}"
                    );
                }
            }
        }

        if wrote && let Some(session_lock) = ctx.session.as_ref() {
            let guard = rt.block_on(tokio::time::timeout(
                std::time::Duration::from_secs(5),
                session_lock.write(),
            ));
            if let Ok(mut session) = guard {
                session.mark_modified(path);
            }
        }

        Ok(output)
    })
}

/// Handle `op="replace_symbol"` by translating to `sobre_refactor`'s
/// `replace_symbol_body` and dispatching through it. The symbol-resolution,
/// CONFLICT guard and atomic write all live in sobre_refactor — this is a thin,
/// pure-mapping adapter (mapping logic lives in `sobre_patch::symbol`).
fn delegate_replace_symbol(
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<ToolOutput, ErrorData> {
    let refactor_args = crate::tools::sobre_patch::build_refactor_args(args)
        .map_err(|e| ErrorData::invalid_params(e, None))?;

    // Resolve `path` at the boundary when given (the name route resolves its own
    // path inside sobre_refactor). abs_path is unused by the symbol-edit branch but
    // we mirror sobre_refactor's wrapper to keep path resolution identical.
    let has_path = args.get("path").and_then(Value::as_str).is_some();
    let abs_path = if has_path {
        require_resolved_path(ctx, args, "path")?
    } else {
        String::new()
    };

    let args_value = Value::Object(refactor_args);
    let result = crate::tools::sobre_refactor::handle(&args_value, &ctx.project_root, &abs_path);

    Ok(ToolOutput {
        text: result,
        original_tokens: 0,
        saved_tokens: 0,
        shell_outcome: None,
        content_blocks: None,
    })
}

/// Resolve `find`/`replace` for replace_all, failing *closed* on the
/// destructive path: a missing `replace` is an error, not "delete every
/// match", and a key of the other ops is named. An empty deletion must be
/// opted into explicitly with `replace=""`.
fn resolve_find_replace(args: &Map<String, Value>) -> Result<(String, String), String> {
    let find = get_str(args, "find")
        .filter(|s| !s.is_empty())
        .ok_or("replace_all requires non-empty 'find'")?;

    for foreign in ["new_text", "old_text"] {
        if args.contains_key(foreign) {
            return Err(format!(
                "replace_all takes 'find' and 'replace', not '{foreign}'"
            ));
        }
    }

    let replace = args
        .get("replace")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or(
            "replace_all requires 'replace' (the replacement text); pass replace=\"\" \
             explicitly to delete every match",
        )?;

    Ok((find, replace))
}

/// Bulk literal find-and-replace — no anchors needed.
fn handle_replace_all(
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<ToolOutput, ErrorData> {
    let (find, replace) =
        resolve_find_replace(args).map_err(|e| ErrorData::invalid_params(e, None))?;
    replace_text(args, ctx, find, replace, true)
}

#[cfg(test)]
mod replace_all_tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => panic!("expected object"),
        }
    }

    #[test]
    fn resolves_find_and_replace() {
        let (f, r) = resolve_find_replace(&obj(json!({"find": "a", "replace": "b"}))).unwrap();
        assert_eq!((f.as_str(), r.as_str()), ("a", "b"));
    }

    #[test]
    fn explicit_empty_replace_is_a_deletion() {
        let (_f, r) = resolve_find_replace(&obj(json!({"find": "a", "replace": ""}))).unwrap();
        assert_eq!(r, "");
    }

    #[test]
    fn missing_replace_is_rejected_not_silent_delete() {
        let err = resolve_find_replace(&obj(json!({"find": "a"}))).unwrap_err();
        assert!(err.contains("requires 'replace'"), "got: {err}");
    }

    #[test]
    fn foreign_replacement_key_is_rejected() {
        for key in ["new_text", "old_text"] {
            let err = resolve_find_replace(&obj(json!({"find": "a", key: "b"}))).unwrap_err();
            assert!(
                err.contains(key),
                "must name the offending key {key}: {err}"
            );
        }
    }

    #[test]
    fn empty_find_is_rejected() {
        let err = resolve_find_replace(&obj(json!({"find": "", "replace": "b"}))).unwrap_err();
        assert!(err.contains("find"), "got: {err}");
    }
}

#[cfg(test)]
mod batch_grouping_tests {
    use super::*;
    use serde_json::json;

    /// A batch spanning two files groups ops per file, preserving the
    /// first-seen file order — no top-level `path` needed.
    #[test]
    fn groups_ops_across_files_preserving_order() {
        let ops = vec![
            json!({"op":"insert_after","path":"a.go","line":1,"hash":"aa","new_text":"x"}),
            json!({"op":"insert_after","path":"b.go","line":2,"hash":"bb","new_text":"y"}),
            json!({"op":"set_line","path":"a.go","line":3,"hash":"cc","new_text":"z"}),
        ];
        let g = group_ops_by_path(&ops, None).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "a.go");
        assert_eq!(g[0].1.len(), 2);
        assert_eq!(g[1].0, "b.go");
        assert_eq!(g[1].1.len(), 1);
    }

    #[test]
    fn ops_without_path_fall_back_to_top_level() {
        let ops = vec![json!({"op":"set_line","line":1,"hash":"aa","new_text":"x"})];
        let g = group_ops_by_path(&ops, Some("top.go")).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].0, "top.go");
    }

    #[test]
    fn op_without_path_and_no_top_level_is_rejected() {
        let ops = vec![json!({"op":"set_line","line":1,"hash":"aa","new_text":"x"})];
        let err = group_ops_by_path(&ops, None).unwrap_err();
        assert!(err.contains("path"), "got: {err}");
    }

    #[test]
    fn empty_ops_rejected() {
        let err = group_ops_by_path(&[], None).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn replace_unique_requires_explicit_old_and_new_text() {
        assert!(unique_texts(&Map::new()).is_err());
        let only_old = Map::from_iter([("old_text".into(), json!("old"))]);
        assert!(unique_texts(&only_old).is_err());
        // The other tools' names are not aliases.
        let aliases = Map::from_iter([
            ("old_string".into(), json!("old")),
            ("new_string".into(), json!("new")),
        ]);
        assert!(unique_texts(&aliases).is_err());
    }
}
