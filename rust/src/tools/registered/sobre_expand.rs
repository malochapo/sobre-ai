use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput};
use crate::tool_defs::tool_def;

pub struct SobreExpandTool;

impl McpTool for SobreExpandTool {
    fn name(&self) -> &'static str {
        "sobre_expand"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_expand",
            "Full output that compression elided. A compressed output's footer names its id \
             → sobre_expand(id=ID). Optional start_line/end_line (1-indexed, inclusive) or \
             search (substring) return a slice. Saved outputs expire after 24h.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Id from the compressed output's footer" },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" },
                    "search": { "type": "string", "description": "Return only lines containing this substring" }
                },
                "required": ["id"]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let args_val = Value::Object(args.clone());
        let result = crate::tools::sobre_expand::handle(&args_val);
        Ok(ToolOutput::simple(result))
    }
}
