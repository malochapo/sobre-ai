//! Runtime validation of published action-conditional tool schemas.

use std::collections::HashSet;

use serde_json::{Value, json};

use super::sobre_expand::SobreExpandTool;
use super::sobre_graph::SobreGraphTool;
use super::sobre_knowledge::SobreKnowledgeTool;
use super::sobre_patch::SobrePatchTool;
use super::sobre_search::SobreSearchTool;
use crate::server::tool_trait::McpTool;

fn validator(tool: &dyn McpTool) -> jsonschema::Validator {
    let schema = Value::Object((*tool.tool_def().input_schema).clone());
    jsonschema::validator_for(&schema).expect("published tool schema must compile")
}

#[test]
fn expand_and_graph_require_action_inputs() {
    let expand = validator(&SobreExpandTool);
    assert!(expand.is_valid(&json!({"id":"cargo_test_1a2b3c4d.log"})));
    assert!(expand.is_valid(&json!({"id":"x.log","start_line":1,"end_line":9})));
    assert!(!expand.is_valid(&json!({})));
    assert!(!expand.is_valid(&json!({"search":"ERROR"})));

    let graph = validator(&SobreGraphTool);
    assert!(graph.is_valid(&json!({"action":"status"})));
    assert!(graph.is_valid(&json!({"action":"path","path":"a","to":"b"})));
    assert!(!graph.is_valid(&json!({"action":"symbol"})));
    assert!(graph.is_valid(&json!({"action":"symbol","symbol":"f"})));
    assert!(graph.is_valid(&json!({"action":"symbol","path":"a.rs::f"})));
    assert!(!graph.is_valid(&json!({"action":"related"})));
    assert!(!graph.is_valid(&json!({"action":"path","path":"a"})));
    assert!(graph.is_valid(&json!({"action":"callers","symbol":"f"})));
    assert!(graph.is_valid(&json!({"action":"callees","symbol":"f","path":"a.rs","depth":2})));
    assert!(graph.is_valid(&json!({"action":"trace","symbol":"a","to":"b"})));
    assert!(!graph.is_valid(&json!({})));
    assert!(!graph.is_valid(&json!({"action":"callers"})));
    assert!(!graph.is_valid(&json!({"action":"trace","symbol":"a"})));
    assert!(!graph.is_valid(&json!({"action":"trace","to":"b"})));
}

#[test]
fn knowledge_search_and_execute_require_mode_specific_inputs() {
    let knowledge = validator(&SobreKnowledgeTool);
    assert!(knowledge.is_valid(&json!({"action":"remember","category":"decision","value":"v"})));
    assert!(knowledge.is_valid(&json!({"action":"recall"})));
    // `value` alone is the documented form; the value is what is required.
    assert!(knowledge.is_valid(&json!({"action":"remember","value":"v"})));
    assert!(!knowledge.is_valid(&json!({"action":"remember","category":"decision"})));
    assert!(!knowledge.is_valid(&json!({"action":"gotcha","trigger":"t"})));

    let search = validator(&SobreSearchTool);
    assert!(search.is_valid(&json!({"pattern":"needle"})));
    assert!(search.is_valid(&json!({"action":"symbol","handle":"f.rs#f@L1"})));
    assert!(!search.is_valid(&json!({})));
    assert!(!search.is_valid(&json!({"action":"semantic"})));
}

/// Collect every declared `properties` key anywhere in a schema (top level and
/// nested — object params, `if`/`then` applicators, array `items`).
fn collect_property_names(node: &Value, out: &mut HashSet<String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::Object(props)) = map.get("properties") {
                out.extend(props.keys().cloned());
            }
            for v in map.values() {
                collect_property_names(v, out);
            }
        }
        Value::Array(arr) => arr.iter().for_each(|v| collect_property_names(v, out)),
        _ => {}
    }
}

/// Collect every string listed in any `required` array anywhere in a schema —
/// including the `then.required` of conditional (`if`/`then`) subschemas.
fn collect_required(node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::Array(req)) = map.get("required") {
                out.extend(req.iter().filter_map(|r| r.as_str().map(str::to_string)));
            }
            for v in map.values() {
                collect_required(v, out);
            }
        }
        Value::Array(arr) => arr.iter().for_each(|v| collect_required(v, out)),
        _ => {}
    }
}

/// Generic schema-integrity guard: once per-op requirements live in the
/// schema (as `if`/`then` conditionals) rather than only in imperative parser
/// code, the schema IS the source of truth — and requiring a param the schema
/// never declares is a self-contradiction a client hits before the handler runs.
/// Assert, for every registered tool, that every `required` token is a declared
/// property. A `then: {required:[…]}` naming a parameter that is no longer
/// declared fails here, for all tools, with no per-tool hardcoding.
#[test]
fn every_required_param_is_a_declared_property() {
    let registry = crate::server::registry::build_registry();
    let mut violations = Vec::new();

    for def in registry.tool_defs() {
        let schema = Value::Object((*def.input_schema).clone());
        let mut props = HashSet::new();
        collect_property_names(&schema, &mut props);
        let mut required = Vec::new();
        collect_required(&schema, &mut required);
        for r in required {
            if !props.contains(&r) {
                violations.push(format!(
                    "{}: schema requires `{r}` but declares no such property",
                    def.name
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "tool schemas require undeclared params (retired/typo'd field?):\n  {}",
        violations.join("\n  ")
    );
}

/// The sharp sobre_patch schema encodes per-op required params, so a
/// client knows before calling that `replace_lines` needs `new_text` — not the
/// retired `new_body`. Exercise the conditionals directly.
#[test]
fn patch_schema_encodes_per_op_required_params() {
    let patch = validator(&SobrePatchTool);

    // set_line requires new_text; the retired new_body does not satisfy it.
    assert!(
        patch.is_valid(&json!({"op":"set_line","path":"a","line":1,"hash":"aa","new_text":"x"}))
    );
    assert!(
        !patch.is_valid(&json!({"op":"set_line","path":"a","line":1,"hash":"aa","new_body":"x"}))
    );

    // replace_lines requires all four anchors + new_text.
    assert!(patch.is_valid(&json!({
        "op":"replace_lines","path":"a",
        "start_line":1,"start_hash":"aa","end_line":2,"end_hash":"bb","new_text":"y"
    })));
    assert!(!patch.is_valid(&json!({
        "op":"replace_lines","path":"a",
        "start_line":1,"start_hash":"aa","end_line":2,"end_hash":"bb"
    })));

    // create/replace_all bodies.
    assert!(!patch.is_valid(&json!({"op":"create","path":"a"})));
    assert!(patch.is_valid(&json!({"op":"create","path":"a","new_text":""})));
    assert!(!patch.is_valid(&json!({"op":"replace_all","path":"a","find":"x"})));
    assert!(patch.is_valid(&json!({"op":"replace_all","path":"a","find":"x","replace":"y"})));

    // Conditionals stay dormant for batch calls: `op` lives inside ops[], so no
    // top-level `if op==…` fires and the batch validates without body params.
    assert!(patch.is_valid(&json!({
        "ops":[{"op":"set_line","path":"a","line":1,"hash":"aa","new_text":"x"}]
    })));
}

/// Decision 7: every argument a handler reads is in its tool's schema, so
/// what `tools/list` shows is everything the tool honours (invariant 3). The
/// sources are each tool's handler and the modules it hands its arguments to.
#[test]
fn every_argument_a_handler_reads_is_declared() {
    let sources: &[(&str, &[&str])] = &[
        ("sobre_checkpoint", &[include_str!("sobre_checkpoint.rs")]),
        ("sobre_compose", &[include_str!("sobre_compose.rs")]),
        (
            "sobre_expand",
            &[
                include_str!("sobre_expand.rs"),
                include_str!("../sobre_expand.rs"),
            ],
        ),
        (
            "sobre_glob",
            &[
                include_str!("sobre_glob.rs"),
                include_str!("../../server/multi_path.rs"),
            ],
        ),
        ("sobre_graph", &[include_str!("sobre_graph.rs")]),
        ("sobre_knowledge", &[include_str!("sobre_knowledge.rs")]),
        ("sobre_outline", &[include_str!("sobre_outline.rs")]),
        (
            "sobre_patch",
            &[
                include_str!("sobre_patch.rs"),
                include_str!("../sobre_patch/symbol.rs"),
            ],
        ),
        (
            "sobre_read",
            &[include_str!("sobre_read.rs"), include_str!("batch_read.rs")],
        ),
        (
            "sobre_refactor",
            &[
                include_str!("sobre_refactor.rs"),
                include_str!("../sobre_refactor/mod.rs"),
            ],
        ),
        ("sobre_review", &[include_str!("sobre_review.rs")]),
        (
            "sobre_search",
            &[
                include_str!("sobre_search.rs"),
                include_str!("../../server/multi_path.rs"),
            ],
        ),
        ("sobre_session", &[include_str!("sobre_session.rs")]),
        ("sobre_shell", &[include_str!("sobre_shell.rs")]),
        ("sobre_tree", &[include_str!("sobre_tree.rs")]),
    ];
    let reads = regex::Regex::new(
        r#"(?:get_(?:str|bool|int|f64|usize|u64|str_array)\(\s*args\s*,\s*|args\.get\(\s*|args\.contains_key\(\s*|str_field\(\s*args\s*,\s*|require_resolved_path\(\s*ctx\s*,\s*args\s*,\s*|ctx\s*\.resolved_path\(\s*|ctx\s*\.path_error\(\s*)"([a-z_]+)""#,
    )
    .unwrap();
    let registry = crate::server::registry::build_registry();
    let mut seen = HashSet::new();
    for (tool, files) in sources {
        let def = registry
            .get_arc(tool)
            .unwrap_or_else(|| panic!("{tool} is not registered"))
            .tool_def();
        let declared: HashSet<&str> = def
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|p| p.keys().map(String::as_str).collect())
            .unwrap_or_default();
        for src in *files {
            for key in reads.captures_iter(src).map(|c| c.get(1).unwrap().as_str()) {
                // Read only under `cfg!(feature = "dense")`, where the schema
                // declares it.
                if *tool == "sobre_search" && key == "mode" && !cfg!(feature = "dense") {
                    continue;
                }
                assert!(
                    declared.contains(key),
                    "{tool} reads `{key}`, which its schema does not declare"
                );
            }
        }
        seen.insert(*tool);
    }
    assert_eq!(
        seen.len(),
        registry.names().len(),
        "every registered tool is checked"
    );
}
