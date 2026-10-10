use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str};
use crate::tool_defs::tool_def;

pub struct SobreComposeTool;

/// The project `path` names, or the session's project root.
fn project_path(ctx: &ToolContext) -> Result<String, ErrorData> {
    if let Some(p) = ctx.resolved_path("path") {
        Ok(p.to_string())
    } else if let Some(err) = ctx.path_error("path") {
        Err(ErrorData::invalid_params(format!("path: {err}"), None))
    } else {
        Ok(ctx.project_root.clone())
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
             Omit task at session start: returns the project overview — structure, hotspots, last task, recent decisions, facts from previous sessions.\n\
             With a task: returns ranked files with relevant symbol source inline grouped by file, and the recorded pitfalls it matches.\n\
             Combines BM25 lexical+semantic retrieval+submodular optimization.\n\
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
        let path = project_path(ctx)?;
        // No task: the caller is orienting, not asking. The session-start
        // overview: structure, hotspots, and what earlier sessions left.
        let Some(task) = get_str(args, "task") else {
            let root = (!path.is_empty()).then_some(path.as_str());
            return Ok(ToolOutput::simple(crate::tools::impl_overview::handle(
                root,
            )));
        };

        // Share the resident BM25 cache with the composed semantic search.
        if let Some(ref cache) = ctx.bm25_cache {
            crate::tools::impl_semantic_search::set_thread_cache(cache.clone());
        }

        let (text, _) = tokio::task::block_in_place(|| {
            crate::tools::sobre_compose::handle(&task, &path, ctx.crp_mode)
        });

        if text.starts_with("ERROR") {
            return Err(ErrorData::invalid_params(text, None));
        }

        Ok(ToolOutput::simple(text))
    }
}
