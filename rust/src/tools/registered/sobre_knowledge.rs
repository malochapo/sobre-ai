use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_f64, get_str};
use crate::tool_defs::tool_def;

pub struct SobreKnowledgeTool;

impl McpTool for SobreKnowledgeTool {
    fn name(&self) -> &'static str {
        "sobre_knowledge"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_knowledge",
            "Facts that outlive the session: decisions, conventions, pitfalls.\n\
             remember value= saves one (key derived when omitted; a new value for a key replaces \
             the old); recall query= finds them (bare: the recent ones); search query= also \
             searches saved sessions; remove deletes one; status lists categories. consolidate \
             imports this session's findings and decisions. gotcha trigger= resolution= records \
             a pitfall, which sobre_compose shows for a matching task.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["remember", "recall", "search", "status", "remove", "consolidate", "gotcha"]
                    },
                    "category": { "type": "string", "description": "Groups facts (remember: default general; gotcha: default convention)" },
                    "key": { "type": "string" },
                    "value": { "type": "string" },
                    "confidence": { "type": "number", "description": "remember: 0.0-1.0 (default 0.8)" },
                    "query": { "type": "string" },
                    "mode": { "type": "string", "description": "recall: auto|exact|semantic|hybrid" },
                    "as_of": { "type": "string", "description": "recall: the facts valid at this date (YYYY-MM-DD)" },
                    "trigger": { "type": "string", "description": "gotcha: when it bites" },
                    "resolution": { "type": "string", "description": "gotcha: the fix" },
                    "severity": { "type": "string", "description": "gotcha: critical|warning|info" },
                    "dry_run": { "type": "boolean", "description": "consolidate: preview only" }
                },
                "allOf": [
                    {
                        "if": { "properties": { "action": { "const": "remember" } }, "required": ["action"] },
                        "then": { "required": ["value"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "remove" } }, "required": ["action"] },
                        "then": { "required": ["category", "key"] }
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
        let value = get_str(args, "value");
        let query = get_str(args, "query");
        let mode = get_str(args, "mode");
        let as_of = get_str(args, "as_of");
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
            if let Err(e) = store.save(&project_root) {
                return Ok(text_output(format!("ERROR: the gotcha was not saved: {e}")));
            }
            return Ok(text_output(msg));
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

        let result = crate::tools::sobre_knowledge::handle(
            &project_root,
            &action,
            category.as_deref(),
            key.as_deref(),
            value.as_deref(),
            query.as_deref(),
            &session_id,
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
    ToolOutput::simple(text)
}
