use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_bool, get_str};
use crate::tool_defs::tool_def;

pub struct SobreSessionTool;

impl McpTool for SobreSessionTool {
    fn name(&self) -> &'static str {
        "sobre_session"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action` (B29).
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_session",
            "Session memory. save at session end, load at start, status = snapshot;\n\
             task|finding|decision record progress (value=text).\n\
             export returns the session as a redacted JSON bundle (write=true or path= also \
             saves it); import path= loads one.\n\
             ANTIPATTERN: permanent project knowledge → sobre_knowledge.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "status", "task", "finding", "decision", "save", "load", "list",
                            "reset", "cleanup", "resume", "snapshot", "restore", "diff",
                            "export", "import", "output_stats"
                        ],
                        "description": "status: this session. task|finding|decision: record one (value). save: write it to disk. load: a saved session (session_id, or this project's latest). list: this project's saved sessions. reset: save, then start a new one. cleanup: delete sessions older than 7 days. resume: a block to continue from. snapshot: save a compaction snapshot; restore: read one back (session_id, or this project's latest). diff: compare two sessions (value='id_a id_b [json]'). export|import: a JSON bundle (path, format, write). output_stats: how often compressed output kept paths and identifiers intact, this process"
                    },
                    "value": { "type": "string" },
                    "session_id": { "type": "string", "description": "load, restore: omit for this project's latest" },
                    "path": { "type": "string", "description": "export: file to write; import: bundle to read" },
                    "format": { "type": "string", "description": "export: json|summary" },
                    "write": { "type": "boolean", "description": "export: also save the bundle (default location under the data directory)" }
                },
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
        let value = get_str(args, "value");
        let sid = get_str(args, "session_id");
        let format = get_str(args, "format");
        // Resolved like any tool path: against the project and the session's
        // working directory, not the server's.
        let path = ctx
            .resolved_path("path")
            .map(str::to_string)
            .or_else(|| get_str(args, "path"));
        let write = get_bool(args, "write").unwrap_or(false);

        let session_handle = ctx
            .session
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("session not available", None))?;
        let mut session = session_handle.blocking_write();
        let result = crate::tools::sobre_session::handle(
            &mut session,
            &action,
            value.as_deref(),
            sid.as_deref(),
            crate::tools::sobre_session::SessionToolOptions {
                format: format.as_deref(),
                path: path.as_deref(),
                write,
            },
        );

        Ok(ToolOutput {
            text: result,
            original_tokens: 0,
            saved_tokens: 0,
            shell_outcome: None,
            content_blocks: None,
        })
    }
}
