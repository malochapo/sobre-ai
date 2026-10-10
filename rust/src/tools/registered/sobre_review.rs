use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str, get_usize};
use crate::tool_defs::tool_def;

pub struct SobreReviewTool;

impl McpTool for SobreReviewTool {
    fn name(&self) -> &'static str {
        "sobre_review"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_review",
            "What a change to a file touches: the files that depend on it, the call sites of \
             its functions in other files, and its test files.\n\
             Actions: review (one file), diff-review (every file in git diff text, or the \
             uncommitted changes when path is omitted), checklist (review questions). \
             depth=N hops of dependents (default 3; diff-review 2).\n\
             Not a substitute for running the tests.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["review", "diff-review", "checklist"]
                    },
                    "path": {
                        "type": "string",
                        "description": "File path (review/checklist) or git diff text (diff-review; omit for the uncommitted changes)"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Hops of dependents (default 3; diff-review 2)"
                    }
                },
                "required": ["action"],
                "allOf": [
                    {
                        "if": {
                            "properties": { "action": { "const": "review" } },
                            "required": ["action"]
                        },
                        "then": { "required": ["action", "path"] }
                    },
                    {
                        "if": {
                            "properties": { "action": { "const": "checklist" } },
                            "required": ["action"]
                        },
                        "then": { "required": ["action", "path"] }
                    }
                ]
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
        // `path` is a file for review/checklist, resolved as every tool's
        // path is; for diff-review it is diff text, taken as given.
        let path = if action == "diff-review" {
            get_str(args, "path")
        } else {
            ctx.resolved_path("path")
                .map(str::to_string)
                .or_else(|| get_str(args, "path"))
        };
        let depth = get_usize(args, "depth").map(|d| d.min(64));
        let result =
            crate::tools::sobre_review::handle(&action, path.as_deref(), &ctx.project_root, depth);

        Ok(ToolOutput::simple(result))
    }
}
