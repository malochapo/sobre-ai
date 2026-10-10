use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str};
use crate::tool_defs::tool_def;

pub struct SobreSessionTool;

impl McpTool for SobreSessionTool {
    fn name(&self) -> &'static str {
        "sobre_session"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_session",
            "This session's task, findings and decisions, saved as they change. status shows \
             them; load brings back a saved session, list names them; resume is a short block \
             to continue from after the host compacts its context. Lasting facts: \
             sobre_knowledge.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "task", "finding", "decision", "load", "list", "resume"],
                        "description": "task|finding|decision record value"
                    },
                    "value": { "type": "string" },
                    "session_id": { "type": "string", "description": "load: omit for this project's latest" }
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
        );

        Ok(ToolOutput::simple(result))
    }
}
