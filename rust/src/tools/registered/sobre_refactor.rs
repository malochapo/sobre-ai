use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, require_resolved_path};
use crate::tool_defs::tool_def;

pub struct SobreRefactorTool;

impl McpTool for SobreRefactorTool {
    fn name(&self) -> &'static str {
        "sobre_refactor"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_refactor",
            "Symbol navigation via the language server: references, definition, \
             implementations, and rename as a preview (nothing is written; apply with sobre_patch).\n\
             Symbol edits work without one: replace_symbol_body, insert_before_symbol, \
             insert_after_symbol resolve the target by name_path (in path's file when path is \
             given) or by path+line.\n\
             ANTIPATTERN: not for symbol discovery — use sobre_search action=symbol, sobre_outline \
             for a file's symbols.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["rename", "references", "definition", "implementations", "replace_symbol_body", "insert_before_symbol", "insert_after_symbol"]
                    },
                    "path": { "type": "string", "description": "File; for symbol edits, the file name_path is looked up in" },
                    "line": { "type": "integer", "description": "1-indexed line; for symbol edits without name_path, the symbol around this line in path" },
                    "column": { "type": "integer", "description": "0-indexed column" },
                    "new_name": { "type": "string", "description": "New symbol name (rename preview)" },
                    "name_path": { "type": "string", "description": "Symbol path for body edits (qualified or bare)" },
                    "new_text": { "type": "string", "description": "Full replacement declaration text (replace_symbol_body)" },
                    "text": { "type": "string", "description": "Sibling text to insert (auto-indented)" },
                    "expected_hash": { "type": "string", "description": "BLAKE3 hex of current range (TOCTOU guard)" },
                    "dry_run": { "type": "boolean", "description": "Symbol edits: show the diff, write nothing" }
                },
                "allOf": [
                    {
                        "if": { "properties": { "action": { "const": "rename" } }, "required": ["action"] },
                        "then": { "required": ["action", "path", "new_name"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "references" } }, "required": ["action"] },
                        "then": { "required": ["action", "path"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "definition" } }, "required": ["action"] },
                        "then": { "required": ["action", "path"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "implementations" } }, "required": ["action"] },
                        "then": { "required": ["action", "path"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "replace_symbol_body" } }, "required": ["action"] },
                        "then": { "required": ["action", "new_text"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "insert_before_symbol" } }, "required": ["action"] },
                        "then": { "required": ["action", "text"] }
                    },
                    {
                        "if": { "properties": { "action": { "const": "insert_after_symbol" } }, "required": ["action"] },
                        "then": { "required": ["action", "text"] }
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
        // name_path edits resolve their own path; only require/resolve `path`
        // when actually provided (read actions + position-fallback edits).
        let has_path = args.get("path").and_then(Value::as_str).is_some();
        let abs_path = if has_path {
            require_resolved_path(ctx, args, "path")?
        } else {
            String::new()
        };

        let args_value = Value::Object(args.clone());
        let result =
            crate::tools::sobre_refactor::handle(&args_value, &ctx.project_root, &abs_path);

        Ok(ToolOutput::simple(result))
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    use crate::server::tool_trait::McpTool;

    #[test]
    fn schema_advertises_only_working_actions() {
        let schema = serde_json::to_string(&SobreRefactorTool.tool_def()).unwrap();
        for needle in [
            "references",
            "preview",
            "replace_symbol_body",
            "insert_before_symbol",
            "insert_after_symbol",
            "name_path",
            "new_text",
            "expected_hash",
        ] {
            assert!(schema.contains(needle), "schema missing {needle}: {schema}");
        }
    }
}
