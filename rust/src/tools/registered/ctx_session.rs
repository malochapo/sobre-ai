use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_bool, get_str};
use crate::tool_defs::tool_def;

pub struct CtxSessionTool;

impl McpTool for CtxSessionTool {
    fn name(&self) -> &'static str {
        "ctx_session"
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "ctx_session",
            "Session memory. save at session end, load at start, status = snapshot;\n\
             task|finding|decision record progress (value=text).\n\
             ANTIPATTERN: permanent project knowledge → ctx_knowledge.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "status|load|save|task|finding|decision|list|… (invalid action lists all)"
                    },
                    "value": { "type": "string" },
                    "session_id": { "type": "string", "description": "Omit for latest" }
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
        let path = get_str(args, "path");
        let write = get_bool(args, "write").unwrap_or(false);
        let privacy = get_str(args, "privacy");

        let session_handle = ctx
            .session
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("session not available", None))?;
        let mut session = session_handle.blocking_write();
        let result = crate::tools::ctx_session::handle(
            &mut session,
            &action,
            value.as_deref(),
            sid.as_deref(),
            crate::tools::ctx_session::SessionToolOptions {
                format: format.as_deref(),
                path: path.as_deref(),
                write,
                privacy: privacy.as_deref(),
            },
        );

        Ok(ToolOutput {
            text: result,
            original_tokens: 0,
            saved_tokens: 0,
            mode: Some(action),
            path: None,
            shell_outcome: None,
            content_blocks: None,
        })
    }
}
