use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str, get_usize};
use crate::tool_defs::tool_def;

pub struct SobreGraphTool;

impl McpTool for SobreGraphTool {
    fn name(&self) -> &'static str {
        "sobre_graph"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_graph",
            "Code graph: file dependencies and function calls.\n\
             Files: neighbors=imports±direction, impact path=file or type → files that \
             depend on it (depth, default 5), changes → files affected by uncommitted changes, \
             path path→to=dependency chain, related.\n\
             Calls: callers|callees symbol='fn' → every call site with file:line \
             (depth≤5, path scopes to a file); trace symbol→to=call chain.\n\
             symbol symbol='fn' (or path=\"file.rs::fn\") returns the definition. \
             diagram kind=deps|calls (Mermaid). build re-scans; status: is it current.\n\
             For understanding code use sobre_compose FIRST.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["build", "status", "related", "symbol", "impact", "changes", "neighbors", "path", "diagram", "callers", "callees", "trace"]
                    },
                    "path": {
                        "type": "string",
                        "description": "File; file scope for symbol|callers|callees"
                    },
                    "symbol": { "type": "string", "description": "Function (symbol|callers|callees|trace)" },
                    "to": { "type": "string", "description": "Target file (path) or function (trace)" },
                    "depth": { "type": "integer", "description": "Hops; callers|callees 1-5 (default 1), impact|changes (default 5)" },
                    "kind": { "type": "string", "description": "diagram: deps|calls" },
                    "format": { "type": "string", "description": "neighbors|path|impact|changes: text|json" },
                    "project_root": { "type": "string" }
                },
                "required": ["action"],
                "allOf": [
                    { "if": { "properties": { "action": { "enum": ["related", "impact", "neighbors", "path"] } }, "required": ["action"] }, "then": { "required": ["path"] } },
                    { "if": { "properties": { "action": { "const": "symbol" } }, "required": ["action"] }, "then": { "anyOf": [{ "required": ["symbol"] }, { "required": ["path"] }] } },
                    { "if": { "properties": { "action": { "const": "path" } }, "required": ["action"] }, "then": { "required": ["path", "to"] } },
                    { "if": { "properties": { "action": { "enum": ["callers", "callees"] } }, "required": ["action"] }, "then": { "required": ["symbol"] } },
                    { "if": { "properties": { "action": { "const": "trace" } }, "required": ["action"] }, "then": { "required": ["symbol", "to"] } }
                ]
            }),
        )
    }

    /// `format=json` answers are JSON documents: they go out verbatim.
    fn produces_machine_readable(&self, args: Option<&Map<String, Value>>) -> bool {
        let Some(args) = args else { return false };
        get_str(args, "format").is_some_and(|f| f.eq_ignore_ascii_case("json"))
            && matches!(
                get_str(args, "action").as_deref(),
                Some("neighbors" | "path" | "impact" | "changes")
            )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let action = get_str(args, "action")
            .ok_or_else(|| ErrorData::invalid_params("action is required", None))?;

        let root = if let Some(p) = ctx.resolved_path("project_root") {
            p.to_string()
        } else if let Some(err) = ctx.path_error("project_root") {
            return Err(ErrorData::invalid_params(
                format!("project_root: {err}"),
                None,
            ));
        } else {
            ctx.project_root.clone()
        };

        // The call actions read `symbol`, `path` and `to` raw: the dispatcher
        // resolves `path` and `to` as filesystem paths, which would turn a
        // function name into `<root>/name`.
        let symbol = get_str(args, "symbol");
        let calls = match action.as_str() {
            "callers" | "callees" => Some(crate::tools::sobre_graph_calls::callers_or_callees(
                symbol.as_deref(),
                action == "callers",
                get_str(args, "path").as_deref(),
                get_usize(args, "depth").unwrap_or(1),
                &root,
            )),
            "trace" => Some(crate::tools::sobre_graph_calls::trace(
                symbol.as_deref(),
                get_str(args, "to").as_deref(),
                &root,
            )),
            _ => None,
        };
        if let Some(text) = calls {
            return Ok(ToolOutput::simple(text));
        }

        // `diagram` takes a substring filter and `symbol` a `file::symbol` spec
        // or a bare name, neither of which is a filesystem path. A name given
        // in `symbol` makes `path` the file it is looked up in.
        let path = if action == "diagram" {
            get_str(args, "path")
        } else if action == "symbol" {
            match (symbol, get_str(args, "path")) {
                (Some(name), Some(file)) => Some(format!("{file}::{name}")),
                (Some(name), None) => Some(name),
                (None, spec) => spec,
            }
        } else if let Some(p) = ctx.resolved_path("path") {
            Some(p.to_string())
        } else if let Some(err) = ctx
            .path_error("path")
            .filter(|_| get_str(args, "path").is_some())
        {
            return Err(ErrorData::invalid_params(format!("path: {err}"), None));
        } else {
            None
        };

        let depth = get_usize(args, "depth").map(|d| d.min(64));
        let kind = get_str(args, "kind");
        let format = get_str(args, "format");

        // `impact` takes a file or a type name, so the raw `path` is used when
        // the dispatcher could not resolve it as a file.
        let impact = match action.as_str() {
            "impact" => Some(crate::tools::sobre_graph_impact::impact(
                path.as_deref().or(get_str(args, "path").as_deref()),
                &root,
                depth,
                format.as_deref(),
            )),
            "changes" => Some(crate::tools::sobre_graph_impact::changes(
                &root,
                depth,
                format.as_deref(),
            )),
            _ => None,
        };
        if let Some(text) = impact {
            return Ok(ToolOutput::simple(text));
        }
        let to = if let Some(p) = ctx.resolved_path("to") {
            Some(p.to_string())
        } else if let Some(err) = ctx
            .path_error("to")
            .filter(|_| get_str(args, "to").is_some())
        {
            return Err(ErrorData::invalid_params(format!("to: {err}"), None));
        } else {
            None
        };

        let result = crate::tools::sobre_graph::handle(
            &action,
            path.as_deref(),
            &root,
            depth,
            kind.as_deref(),
            to.as_deref(),
            format.as_deref(),
        );

        Ok(ToolOutput::simple(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::data_dir::IsolatedDataDir;

    /// A project where `alpha` calls `beta` and `beta` calls `gamma`, with the
    /// graph index built in a private data dir. Returns (data-dir guard,
    /// project tempdir guard, root).
    fn chain_fixture() -> (IsolatedDataDir, tempfile::TempDir, String) {
        let iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().expect("tempdir");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(
            proj.join("Cargo.toml"),
            "[package]\nname = \"chain\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            proj.join("src/lib.rs"),
            "pub fn alpha() {\n    beta();\n}\n\npub fn beta() {\n    gamma();\n}\n\npub fn gamma() {}\n",
        )
        .unwrap();
        let root = proj.to_string_lossy().to_string();
        crate::core::graph_index::scan(&root);
        (iso, tmp, root)
    }

    /// Run `sobre_graph` the way the dispatcher does: `path` and `to` arrive
    /// pre-resolved against the project root, as for any filesystem path.
    fn call(root: &str, args: &Value) -> String {
        let args = args.as_object().unwrap().clone();
        let mut ctx = ToolContext {
            project_root: root.to_string(),
            cache: Some(std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::cache::SessionCache::new(),
            ))),
            ..ToolContext::default()
        };
        for key in ["path", "to"] {
            if let Some(raw) = args.get(key).and_then(Value::as_str) {
                ctx.resolved_paths
                    .insert(key.to_string(), format!("{root}/{raw}"));
            }
        }
        SobreGraphTool.handle(&args, &ctx).unwrap().text
    }

    #[test]
    fn call_actions_take_function_names_not_paths() {
        let (_iso, _tmp, root) = chain_fixture();

        let callers = call(
            &root,
            &json!({"action": "callers", "symbol": "beta", "path": "src/lib.rs"}),
        );
        assert!(callers.contains("1 caller(s) of 'beta'"), "{callers}");
        assert!(callers.contains("alpha"), "{callers}");
        // `beta();` sits on line 2 of src/lib.rs.
        assert!(
            callers.contains("(L2)"),
            "call sites carry their real line: {callers}"
        );

        let scoped_out = call(
            &root,
            &json!({"action": "callers", "symbol": "beta", "path": "src/other.rs"}),
        );
        assert!(scoped_out.starts_with("No callers found"), "{scoped_out}");

        let callees = call(
            &root,
            &json!({"action": "callees", "symbol": "alpha", "depth": 2}),
        );
        assert!(callees.contains("(depth≤2)"), "{callees}");
        assert!(
            callees.contains("beta") && callees.contains("gamma"),
            "{callees}"
        );

        let trace = call(
            &root,
            &json!({"action": "trace", "symbol": "alpha", "to": "gamma"}),
        );
        assert!(trace.contains("Call path (2 hop(s))"), "{trace}");
        assert!(trace.contains("→ gamma"), "{trace}");
    }

    /// A path-qualified call (`util::helper()`) is a call to `helper`,
    /// and `Type::method` names a symbol, not a file.
    #[test]
    fn path_qualified_calls_and_names_resolve() {
        let (_iso, _tmp, root) = chain_fixture();
        std::fs::write(
            format!("{root}/src/util.rs"),
            "pub fn helper() {}\npub struct Config;\nimpl Config {\n    pub fn load() {}\n}\n",
        )
        .unwrap();
        std::fs::write(
            format!("{root}/src/main.rs"),
            "mod util;\nfn main() {\n    util::helper();\n    util::Config::load();\n}\n",
        )
        .unwrap();
        crate::core::graph_index::scan(&root);

        let callers = call(&root, &json!({"action": "callers", "symbol": "helper"}));
        assert!(callers.contains("src/main.rs → main"), "{callers}");
        let load = call(
            &root,
            &json!({"action": "symbol", "symbol": "Config::load"}),
        );
        assert!(load.contains("pub fn load()"), "{load}");
    }

    /// An edit after the build is in the next answer.
    #[test]
    fn an_edit_after_the_build_is_seen() {
        let (_iso, _tmp, root) = chain_fixture();
        let first = call(&root, &json!({"action": "callers", "symbol": "gamma"}));
        assert!(first.contains("1 caller(s)"), "{first}");

        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(
            format!("{root}/src/extra.rs"),
            "pub fn delta() {\n    crate::gamma();\n}\n",
        )
        .unwrap();
        let after = call(&root, &json!({"action": "callers", "symbol": "gamma"}));
        assert!(after.contains("2 caller(s)"), "{after}");
        assert!(after.contains("src/extra.rs → delta"), "{after}");
    }

    #[test]
    fn symbol_action_resolves_a_bare_name() {
        let (_iso, _tmp, root) = chain_fixture();

        let bare = call(&root, &json!({"action": "symbol", "path": "beta"}));
        assert!(bare.contains("pub fn beta()"), "{bare}");

        let qualified = call(
            &root,
            &json!({"action": "symbol", "path": "src/lib.rs::gamma"}),
        );
        assert!(qualified.contains("pub fn gamma()"), "{qualified}");
    }

    #[test]
    fn symbol_action_takes_the_name_in_symbol() {
        let (_iso, _tmp, root) = chain_fixture();

        let named = call(&root, &json!({"action": "symbol", "symbol": "beta"}));
        assert!(named.contains("pub fn beta()"), "{named}");

        let scoped = call(
            &root,
            &json!({"action": "symbol", "symbol": "gamma", "path": "src/lib.rs"}),
        );
        assert!(scoped.contains("pub fn gamma()"), "{scoped}");

        let elsewhere = call(
            &root,
            &json!({"action": "symbol", "symbol": "gamma", "path": "src/other.rs"}),
        );
        assert!(
            elsewhere.contains("not found in src/other.rs"),
            "{elsewhere}"
        );
    }

    #[test]
    fn json_impact_is_machine_readable_and_parses() {
        let (_iso, _tmp, root) = chain_fixture();
        let args = json!({"action": "impact", "path": "src/lib.rs", "format": "json"});
        assert!(SobreGraphTool.produces_machine_readable(args.as_object()));
        let out = call(&root, &args);
        let v: Value = serde_json::from_str(&out).expect("impact json parses");
        assert_eq!(v["tool"], "sobre_graph");
        assert_eq!(v["action"], "impact");
        let text = json!({"action": "impact", "path": "src/lib.rs"});
        assert!(!SobreGraphTool.produces_machine_readable(text.as_object()));
    }
}
