use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str, require_resolved_path};
use crate::tool_defs::tool_def;

pub struct CtxRefactorTool;

impl McpTool for CtxRefactorTool {
    fn name(&self) -> &'static str {
        "ctx_refactor"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action` (B29).
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "ctx_refactor",
            "Symbol navigation and edits: rename, references, definition, implementations \
             via the language server; symbols_overview and symbol body edits work without one.\n\
             WORKFLOW: use action=references first to find usages before refactoring.\n\
             ANTIPATTERN: not for symbol discovery — use ctx_search action=symbol or ctx_compose.\n\
             Symbol edits (replace_symbol_body, insert_before_symbol, insert_after_symbol) \
             resolve the target by name_path.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "rename|references|definition|implementations|symbols_overview|replace_symbol_body|insert_before_symbol|insert_after_symbol"
                    },
                    "path": { "type": "string", "description": "Path" },
                    "line": { "type": "integer", "description": "1-indexed line" },
                    "column": { "type": "integer", "description": "0-indexed column" },
                    "new_name": { "type": "string", "description": "New symbol name" },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "all"],
                        "description": "project|all"
                    },
                    "name_path": { "type": "string", "description": "Symbol path for body edits (qualified or bare)" },
                    "file": { "type": "string", "description": "Scope name_path resolution to this file (avoids AMBIGUOUS_SYMBOL for common names)" },
                    "new_text": { "type": "string", "description": "Full replacement declaration text (replace_symbol_body)" },
                    "text": { "type": "string", "description": "Sibling text to insert (auto-indented)" },
                    "end_line": { "type": "integer", "description": "1-based last line (path+line fallback)" },
                    "expected_hash": { "type": "string", "description": "BLAKE3 hex of current range (TOCTOU guard)" }
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
                        "if": { "properties": { "action": { "const": "symbols_overview" } }, "required": ["action"] },
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
        let result = crate::tools::ctx_refactor::handle(&args_value, &ctx.project_root, &abs_path);

        let action = get_str(args, "action").unwrap_or_default();
        Ok(ToolOutput {
            text: result,
            original_tokens: 0,
            saved_tokens: 0,
            mode: Some(action.clone()),
            path: get_str(args, "path"),
            shell_outcome: None,
            content_blocks: None,
        })
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    use crate::server::tool_trait::McpTool;

    #[test]
    fn schema_advertises_only_working_actions() {
        let schema = serde_json::to_string(&CtxRefactorTool.tool_def()).unwrap();
        for needle in [
            "references",
            "\"scope\"",
            "symbols_overview",
            "replace_symbol_body",
            "insert_before_symbol",
            "insert_after_symbol",
            "name_path",
            "new_text",
            "expected_hash",
        ] {
            assert!(schema.contains(needle), "schema missing {needle}: {schema}");
        }
        for gone in [
            "rename_preview",
            "move_apply",
            "inline_preview",
            "reformat",
            "type_hierarchy",
        ] {
            assert!(!schema.contains(gone), "schema still lists {gone}");
        }
    }
}
