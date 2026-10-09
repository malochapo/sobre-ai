use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{
    McpTool, ToolContext, ToolOutput, get_f64, get_str, get_str_array,
};
use crate::tool_defs::tool_def;

pub struct SobreKnowledgeTool;

impl McpTool for SobreKnowledgeTool {
    fn name(&self) -> &'static str {
        "sobre_knowledge"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action` (B29).
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_knowledge",
            "Persistent memory across sessions — remember decisions, patterns, and facts for recall.\n\
             WORKFLOW: save after completing significant tasks; recall at session start.\n\
             action=remember value='Y' saves a fact (key optional — derived from value; content= is an accepted alias).\n\
             action=recall query='X' retrieves it (bare recall lists recent facts). action=status shows all categories.\n\
action=consolidate imports this session's findings and decisions, runs lifecycle, then frees 25% facts/history/patterns capacity.\n\
             action=gotcha trigger='X' resolution='Y' for known pitfalls.\n\
             mode=semantic|exact for recall. category groups related facts.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "remember|recall|search|pattern|gotcha|relate|relations|consolidate|restore|status|timeline|rooms|wakeup|remove|export|import (also: feedback, unrelate, relations_diagram, health, lifecycle_report, policy, embeddings_*)"
                    },
                    "trigger": { "type": "string", "description": "gotcha trigger pattern" },
                    "resolution": { "type": "string", "description": "gotcha resolution/fix" },
                    "severity": { "type": "string", "description": "gotcha: critical|warning|info" },
                    "category": { "type": "string", "description": "Fact category (remember: default general)" },
                    "key": { "type": "string" },
                    "value": { "type": "string" },
                    "content": { "type": "string", "description": "Alias for value (remember)" },
                    "query": { "type": "string", "description": "Query for recall/search/relate/restore" },
                    "mode": { "type": "string", "description": "auto|exact|semantic|hybrid" },
                    "as_of": { "type": "string", "description": "YYYY-MM-DD date filter" },
                    "pattern_type": { "type": "string" },
                    "examples": { "type": "array", "items": { "type": "string" } },
                    "confidence": { "type": "number", "description": "0.0-1.0" },
                    "store": { "type": "string", "description": "restore: facts|history|patterns (default: all)" },
                    "limit": { "type": "number", "description": "restore: max items to recover (default 50)" },
                    "dry_run": { "type": "boolean", "description": "consolidate: preview imports/reclaim without writing" },
                    "format": { "type": "string", "description": "export: json|okf (okf = portable Markdown bundle)" },
                    "path": { "type": "string", "description": "export/import: bundle directory (OKF) or file path" },
                    "merge": { "type": "string", "description": "import: replace|append|skip-existing (default skip-existing)" }
                },
                "allOf": [
                    {
                        "if": { "properties": { "action": { "const": "remember" } }, "required": ["action"] },
                        "then": { "anyOf": [{ "required": ["value"] }, { "required": ["content"] }] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "pattern" } }, "required": ["action"] },
                        "then": { "required": ["value"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "search" } }, "required": ["action"] },
                        "then": { "required": ["query"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "gotcha" } }, "required": ["action"] },
                        "then": { "required": ["trigger", "resolution"] }
                    }
                ],
                "required": ["action"]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let action = get_str(args, "action")
            .ok_or_else(|| ErrorData::invalid_params("action is required", None))?;
        let category = get_str(args, "category");
        let key = get_str(args, "key");
        // `content` is the wording our own workflow docs use for remember;
        // accept it as an alias so agents following those docs succeed.
        let value = get_str(args, "value").or_else(|| get_str(args, "content"));
        let query = get_str(args, "query");
        let mode = get_str(args, "mode");
        let as_of = get_str(args, "as_of");
        let pattern_type = get_str(args, "pattern_type");
        let examples = get_str_array(args, "examples");
        let confidence = get_f64(args, "confidence").map(|v| v as f32);

        let session_handle = ctx
            .session
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("session not available", None))?;
        let (session_id, project_root, live_session) = {
            let timeout_dur =
                crate::core::io_health::adaptive_timeout(std::time::Duration::from_secs(10));
            let read_result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(tokio::time::timeout(timeout_dur, session_handle.read()))
            });
            if let Ok(session) = read_result {
                let sid = session.id.clone();
                let root = session
                    .project_root
                    .clone()
                    .unwrap_or_else(|| ctx.project_root.clone());
                // `consolidate` imports this conversation's session as it is
                // now, not as it was last saved.
                let live = (action == "consolidate").then(|| session.clone());
                (sid, root, live)
            } else {
                tracing::warn!("sobre_knowledge: session read-lock timeout, using fallback");
                ("unknown".to_string(), ctx.project_root.clone(), None)
            }
        };

        if action == "gotcha" {
            let trigger = get_str(args, "trigger").unwrap_or_default();
            let resolution = get_str(args, "resolution").unwrap_or_default();
            let severity = get_str(args, "severity").unwrap_or_default();
            let cat = category.as_deref().unwrap_or("convention");

            if trigger.is_empty() || resolution.is_empty() {
                return Ok(text_output(
                    "ERROR: trigger and resolution are required for gotcha action".to_string(),
                ));
            }

            let mut store = crate::core::gotcha_tracker::GotchaStore::load(&project_root);
            let msg = match store.report_gotcha(&trigger, &resolution, cat, &severity, &session_id)
            {
                Some(gotcha) => {
                    let conf = (gotcha.confidence * 100.0) as u32;
                    let label = gotcha.category.short_label();
                    format!("Gotcha recorded: [{label}] {trigger} (confidence: {conf}%)")
                }
                None => {
                    format!("Gotcha noted: {trigger} (evicted by higher-confidence entries)")
                }
            };
            let _ = store.save(&project_root);
            return Ok(text_output(msg));
        }

        // Restore (Phase 6): explicit cross-store undo from archive. Handled
        // inline (like `gotcha`) so `store`/`limit` can be passed without widening
        // the shared `handle()` signature.
        if action == "restore" {
            let store = match get_str(args, "store").as_deref() {
                Some(s) => match crate::core::memory_archive::MemoryStore::parse(s) {
                    Some(ms) => Some(ms),
                    None => {
                        return Ok(text_output(format!(
                            "ERROR: unknown store: {s}. Use: facts, history, patterns"
                        )));
                    }
                },
                None => None,
            };
            let limit = get_f64(args, "limit")
                .map_or(crate::tools::sobre_knowledge::DEFAULT_RESTORE_LIMIT, |v| {
                    v as usize
                });
            let opts =
                crate::tools::sobre_knowledge::RestoreOptions::new(store, query.clone(), limit);
            let text = match crate::tools::sobre_knowledge::run_restore(&project_root, &opts) {
                Ok(report) => crate::tools::sobre_knowledge::format_restore_report(&report),
                Err(e) => e,
            };
            return Ok(text_output(text));
        }

        // `dry_run` previews the imports and the reclaim with no writes.
        if action == "consolidate" {
            let dry_run = args
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let opts = crate::core::consolidation_engine::ConsolidateOptions::manual();
            let opts = if dry_run { opts.into_dry_run() } else { opts };
            let text = match crate::tools::sobre_knowledge::consolidate_project_knowledge_with(
                &project_root,
                live_session.as_ref(),
                &opts,
            ) {
                Ok(report) => crate::tools::sobre_knowledge::format_consolidation_report(&report),
                Err(e) => e,
            };
            return Ok(text_output(text));
        }

        // Export/import handled inline so `format`/`path`/`merge` stay off the
        // shared handle() signature (same pattern as restore/gotcha above).
        // `path` is resolved like any tool path: against the project and the
        // session's working directory, not the server's (B27).
        let io_path = ctx
            .resolved_path("path")
            .map(str::to_string)
            .or_else(|| get_str(args, "path"));
        if action == "export" {
            let text = if get_str(args, "format").as_deref() == Some("okf") {
                crate::tools::sobre_knowledge::handle_export_okf(&project_root, io_path.as_deref())
            } else {
                crate::tools::sobre_knowledge::handle_export(&project_root, io_path.as_deref())
            };
            return Ok(text_output(text));
        }
        if action == "import" {
            let Some(path) = io_path.or_else(|| query.clone()) else {
                return Ok(text_output(
                    "ERROR: import requires `path` (a file or an OKF directory)".to_string(),
                ));
            };
            let merge = get_str(args, "merge")
                .and_then(|s| crate::core::knowledge::ImportMerge::parse(&s))
                .unwrap_or(crate::core::knowledge::ImportMerge::SkipExisting);
            return Ok(text_output(crate::tools::sobre_knowledge::handle_import(
                &project_root,
                &path,
                merge,
                &session_id,
            )));
        }

        let result = crate::tools::sobre_knowledge::handle(
            &project_root,
            &action,
            category.as_deref(),
            key.as_deref(),
            value.as_deref(),
            query.as_deref(),
            &session_id,
            pattern_type.as_deref(),
            examples,
            confidence,
            mode.as_deref(),
            as_of.as_deref(),
        );

        Ok(text_output(result))
    }
}

/// A plain text `ToolOutput`. `sobre_knowledge` results are already
/// compressed prose, so token accounting is left at zero.
fn text_output(text: String) -> ToolOutput {
    ToolOutput {
        text,
        original_tokens: 0,
        saved_tokens: 0,
        shell_outcome: None,
        content_blocks: None,
    }
}
