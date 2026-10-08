use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str, require_resolved_path};
use crate::tool_defs::tool_def;
use crate::tools::sobre_outline::OutlineOpts;

pub struct SobreOutlineTool;

impl McpTool for SobreOutlineTool {
    fn name(&self) -> &'static str {
        "sobre_outline"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action` (B29).
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_outline",
            "WORKFLOW: call BEFORE sobre_read to map code structure (a syntax-aware table of contents).\n\
            Accepts a FILE or a DIRECTORY (folder surface — per-file symbols). Symbols come from\n\
            tree-sitter (27 languages, real line spans); a conservative regex fallback covers the rest.\n\
            kind=fn|struct|class|trait|enum|impl|all filters by kind; match=<substr> filters by name\n\
            (case-insensitive); format=json emits deterministic JSON labelling the backend per file.\n\
            ANTIPATTERN: NOT for file content (use sobre_read) or deep understanding (use sobre_compose).",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File or directory" },
                    "kind": { "type": "string", "description": "Filter by kind: fn|struct|class|trait|enum|impl|all" },
                    "match": { "type": "string", "description": "Keep only symbols whose name contains this (case-insensitive)" },
                    "format": { "type": "string", "description": "Output format: text (default) | json (deterministic)" }
                },
                "required": ["path"]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let path = require_resolved_path(ctx, args, "path")?;
        let kind = get_str(args, "kind");
        let name_match = get_str(args, "match");
        let as_json = get_str(args, "format").as_deref() == Some("json");

        let (result, original) = crate::tools::sobre_outline::run(
            &path,
            &OutlineOpts {
                kind: kind.as_deref(),
                name_match: name_match.as_deref(),
                as_json,
            },
        );
        let sent = crate::core::tokens::count_tokens(&result);
        let saved = original.saturating_sub(sent);

        Ok(ToolOutput {
            text: result,
            original_tokens: original,
            saved_tokens: saved,
            mode: kind,
            path: Some(path),
            shell_outcome: None,
            content_blocks: None,
        })
    }

    /// `format=json` produces a deterministic, byte-stable JSON document:
    /// it must be returned verbatim, so the dispatch pipeline skips all prose
    /// decorations and compression for it.
    fn produces_machine_readable(&self, args: Option<&Map<String, Value>>) -> bool {
        args.and_then(|a| a.get("format")).and_then(Value::as_str) == Some("json")
    }
}
