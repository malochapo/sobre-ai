use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str};
use crate::tool_defs::tool_def;

pub struct SobreComposeTool;

impl SobreComposeTool {
    /// `sobre_compose` with no `task`: the session-start orientation pass.
    ///
    /// Structure, project markers, the last task, recent decisions and the
    /// facts carried over from previous sessions. Distinct from the ranked
    /// retrieval `compose` performs once a task is known — at session start
    /// there is no task yet, which is exactly when this is needed.
    fn overview(args: &Map<String, Value>, ctx: &ToolContext) -> Result<ToolOutput, ErrorData> {
        let resolved_path = if get_str(args, "path").is_some() {
            if let Some(p) = ctx.resolved_path("path") {
                Some(p.to_string())
            } else if let Some(err) = ctx.path_error("path") {
                return Err(ErrorData::invalid_params(format!("path: {err}"), None));
            } else {
                None
            }
        } else if let Some(ref session) = ctx.session {
            let guard =
                crate::server::bounded_lock::read(session, "sobre_compose:overview:session");
            guard.as_ref().and_then(|g| g.project_root.clone())
        } else {
            None
        };

        let cache = ctx
            .cache
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("cache not available", None))?;
        let Some(guard) = crate::server::bounded_lock::read(cache, "sobre_compose:overview:cache")
        else {
            return Ok(ToolOutput::simple(
                "[overview temporarily unavailable — cache busy]".to_string(),
            ));
        };

        let (result, _complete) = crate::tools::impl_overview::handle(
            &guard,
            None,
            resolved_path.as_deref(),
            ctx.crp_mode,
        );

        Ok(ToolOutput {
            text: result,
            original_tokens: 0,
            saved_tokens: 0,
            mode: Some("overview".to_string()),
            path: None,
            shell_outcome: None,
            content_blocks: None,
        })
    }
}

impl McpTool for SobreComposeTool {
    fn name(&self) -> &'static str {
        "sobre_compose"
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_compose",
            "PRIMARY TOOL — call FIRST for understanding code (before editing/debugging/'how does X work').\n\
             Omit task at session start: returns the project overview — structure, markers, last task, recent decisions, facts from previous sessions.\n\
             With a task: returns ranked files with relevant symbol source inline grouped by file.\n\
             Combines BM25 lexical+semantic+associative retrieval+submodular optimization.\n\
             ANTIPATTERN: Do NOT chain search→read→symbol — one compose replaces the whole chain.\n\
             ANTIPATTERN: Do NOT Read files whose source compose already returned — it IS the source.\n\
             WORKFLOW: Fire parallel sobre_read or sobre_compose for different areas.",
            json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "Short English task/question or symbol names. Omit for the session-start project overview." },
                    "path": { "type": "string", "description": "Project root" }
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
        // No task: the caller is orienting, not asking. Return the project
        // overview — structure, markers, last task, recent decisions and the
        // facts carried over from previous sessions. `task` is what turns
        // compose from "where am I" into "what do I need for this".
        let Some(task) = get_str(args, "task") else {
            return Self::overview(args, ctx);
        };
        let path = if let Some(p) = ctx.resolved_path("path") {
            p.to_string()
        } else if let Some(err) = ctx.path_error("path") {
            return Err(ErrorData::invalid_params(format!("path: {err}"), None));
        } else {
            ctx.project_root.clone()
        };

        // Share the resident BM25 cache with the composed semantic search.
        if let Some(ref cache) = ctx.bm25_cache {
            crate::tools::impl_semantic_search::set_thread_cache(cache.clone());
        }

        let (text, sent) = tokio::task::block_in_place(|| {
            crate::tools::sobre_compose::handle(&task, &path, ctx.crp_mode)
        });

        if text.starts_with("ERROR") {
            return Err(ErrorData::invalid_params(text, None));
        }

        Ok(ToolOutput {
            text,
            original_tokens: sent,
            saved_tokens: 0,
            mode: Some("compose".to_string()),
            path: Some(path),
            shell_outcome: None,
            content_blocks: None,
        })
    }
}
