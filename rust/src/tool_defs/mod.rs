use std::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Map, Value};

pub fn tool_def(name: &'static str, description: &'static str, schema_value: Value) -> Tool {
    let mut schema: Map<String, Value> = match schema_value {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    normalize_for_strict_validators(&mut schema);
    Tool::new(name, description, Arc::new(schema))
}

/// Tools that never mutate their environment (files, indexes, session state).
/// MCP clients (Cursor, Claude Desktop) may use `readOnlyHint` to allow these
/// tools in restricted/readonly subagent contexts.
pub const READONLY_TOOL_NAMES: &[&str] = &[
    "ctx_read",
    "ctx_compose",
    "ctx_search",
    "ctx_tree",
    "ctx_glob",
    "ctx_callgraph",
    "ctx_expand",
];

/// Tools that may destructively modify their environment.
pub const DESTRUCTIVE_TOOL_NAMES: &[&str] = &["ctx_shell", "ctx_patch"];

/// Apply MCP `ToolAnnotations` (readOnlyHint, destructiveHint) to a set of
/// tool definitions. Called by the registry before serving `tools/list`.
pub fn apply_tool_annotations(tools: Vec<Tool>) -> Vec<Tool> {
    tools
        .into_iter()
        .map(|t| {
            let name = t.name.as_ref();
            if READONLY_TOOL_NAMES.contains(&name) {
                t.annotate(
                    ToolAnnotations::new()
                        .read_only(true)
                        .destructive(false)
                        .idempotent(true),
                )
            } else if DESTRUCTIVE_TOOL_NAMES.contains(&name) {
                t.annotate(ToolAnnotations::new().destructive(true))
            } else {
                t
            }
        })
        .collect()
}

/// Make a tool input schema acceptable to *strict* JSON-Schema validators.
///
/// OpenAI/Azure (Pydantic-based), Claude thinking models and OpenAI-compatible
/// backends like SGLang reject tool schemas that are valid JSON Schema but
/// omit fields the spec treats as optional. Community-reported failures
/// (OpenCode: "Invalid schema for function 'lean-ctx_ctx_expand': None is not
/// of type 'array'"):
///
/// - `type: "object"` with `properties` but no `required` → clients forward
///   `required: null` and the backend 400s. We always emit an explicit array.
/// - `type: "array"` without `items` → "array schema missing items". We emit
///   a permissive `items: {}` so the wire schema is self-contained.
///
/// Runs recursively over every nested schema position (`properties`, `items`,
/// `anyOf`/`oneOf`/`allOf`, object-shaped `additionalProperties`) so nested
/// definitions get the same guarantees. Existing `required` arrays are
/// preserved verbatim — this never changes which parameters are mandatory.
pub fn normalize_for_strict_validators(schema: &mut Map<String, Value>) {
    let is_object = schema.get("type").and_then(Value::as_str) == Some("object");
    let is_array = schema.get("type").and_then(Value::as_str) == Some("array");

    if is_object && schema.contains_key("properties") && !schema.contains_key("required") {
        schema.insert("required".into(), Value::Array(Vec::new()));
    }
    if is_array && !schema.contains_key("items") {
        schema.insert("items".into(), Value::Object(Map::new()));
    }

    if let Some(Value::Object(props)) = schema.get_mut("properties") {
        for prop in props.values_mut() {
            if let Value::Object(p) = prop {
                normalize_for_strict_validators(p);
            }
        }
    }
    if let Some(Value::Object(items)) = schema.get_mut("items") {
        normalize_for_strict_validators(items);
    }
    if let Some(Value::Object(ap)) = schema.get_mut("additionalProperties") {
        normalize_for_strict_validators(ap);
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(branches)) = schema.get_mut(combinator) {
            for branch in branches.iter_mut() {
                if let Value::Object(b) = branch {
                    normalize_for_strict_validators(b);
                }
            }
        }
    }
    for keyword in ["if", "then", "else", "not"] {
        if let Some(Value::Object(sub)) = schema.get_mut(keyword) {
            normalize_for_strict_validators(sub);
        }
    }
}
