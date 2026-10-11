//! Generated reference documentation.
//!
//! Renders Markdown appendices directly from the in-code single sources of
//! truth (the MCP tool registry and the `Config` schema) so the published
//! reference can never drift from the actual feature surface.
//!
//! - `mcp-tools.md`   — every registered MCP tool, from `manifest_value()`.
//! - `config-keys.md` — every recognized `config.toml` key, from `ConfigSchema`.
//! - `cli.md`         — every command and flag, from the clap declarations
//!   in `cli::args`.
//! - `env-vars.md`    — every environment variable, from `core::env_vars`.
//!
//! Used by the `gen_docs` example (writes the files) and by drift tests /
//! the CI gate (compare on-disk vs. freshly rendered).

use std::path::PathBuf;

use serde_json::Value;

use crate::core::config::schema::ConfigSchema;

const DO_NOT_EDIT: &str = "<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->";

/// Directory the generated reference docs live in (`docs/reference/generated`).
pub fn generated_dir() -> PathBuf {
    let rust_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = rust_dir.parent().unwrap_or(&rust_dir);
    repo_root.join("docs/reference/generated")
}

/// Every generated reference document as `(filename, markdown)` pairs.
/// This is the canonical list shared by the writer and the drift tests.
pub fn generated_docs() -> Vec<(&'static str, String)> {
    vec![
        ("mcp-tools.md", mcp_tools_markdown()),
        ("config-keys.md", config_keys_markdown()),
        ("cli.md", crate::cli::reference::markdown()),
        ("env-vars.md", crate::core::env_vars::markdown()),
    ]
}

/// True when on-disk content equals freshly generated content, ignoring
/// line-ending differences. Windows checkouts may store the committed docs
/// with CRLF while the generator emits LF; the drift gate compares *content*,
/// not byte-exact line endings.
pub fn content_matches(on_disk: &str, generated: &str) -> bool {
    normalize_newlines(on_disk) == normalize_newlines(generated)
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

// ---------------------------------------------------------------------------
// MCP tools
// ---------------------------------------------------------------------------

/// Markdown reference for every registered MCP tool, rendered from the same
/// manifest the editors consume: each parameter with its type, whether it is
/// required, and its description, then the requirements that depend on
/// `action`.
pub fn mcp_tools_markdown() -> String {
    let manifest = crate::core::mcp_manifest::manifest_value();
    let mut tools: Vec<&Value> = manifest
        .get("tools")
        .and_then(|t| t.get("granular"))
        .and_then(|g| g.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    tools.sort_by(|a, b| tool_name(a).cmp(tool_name(b)));

    let mut out = String::new();
    out.push_str("# Appendix — MCP Tools (generated)\n\n");
    out.push_str(DO_NOT_EDIT);
    out.push_str("\n\n");
    out.push_str(
        "Source of truth: `rust/src/server/registry.rs` and the tool definitions it registers.\n\n",
    );
    out.push_str(&format!(
        "sobre registers **{} MCP tools**, the same list in every session \
         (invariant 3). This page shows the build without local embeddings; \
         the build with them adds one parameter, `sobre_search`'s `mode` \
         (`bm25|dense|hybrid`; hybrid when omitted and the model is \
         installed). A handler reads no argument its schema does not declare \
         (a test enforces it).\n\n",
        tools.len()
    ));

    for tool in tools {
        let name = tool_name(tool);
        out.push_str(&format!("## `{name}`\n\n"));

        let desc = tool
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim();
        if !desc.is_empty() {
            out.push_str(desc);
            out.push_str("\n\n");
        }

        let schema = tool.get("input_schema");
        let params = render_tool_params(schema);
        if params.is_empty() {
            out.push_str("Parameters: _none_\n\n");
        } else {
            out.push_str(&params);
            out.push('\n');
        }
        let conditions = render_conditions(schema);
        if !conditions.is_empty() {
            out.push_str(&conditions);
            out.push('\n');
        }
    }
    out
}

fn tool_name(tool: &Value) -> &str {
    tool.get("name").and_then(|n| n.as_str()).unwrap_or("")
}

/// A parameter's type as written in the reference: `string`, `array of
/// string`, `one of a, b`.
fn param_type(prop: &Value) -> String {
    if let Some(values) = prop.get("enum").and_then(Value::as_array) {
        let values: Vec<String> = values
            .iter()
            .map(|v| {
                format!(
                    "`{}`",
                    v.as_str().map_or_else(|| v.to_string(), str::to_string)
                )
            })
            .collect();
        return format!("one of {}", values.join(", "));
    }
    let ty = prop.get("type").and_then(Value::as_str).unwrap_or("any");
    if ty == "array" {
        let item = prop
            .get("items")
            .and_then(|i| i.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("object");
        return format!("array of {item}");
    }
    ty.to_string()
}

/// The parameter table of a tool, sorted, required ones first.
fn render_tool_params(schema: Option<&Value>) -> String {
    let props = schema
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.as_object());
    let Some(props) = props else {
        return String::new();
    };
    let required: Vec<&str> = schema
        .and_then(|s| s.get("required"))
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut names: Vec<&String> = props.keys().collect();
    names.sort_by_key(|n| (!required.contains(&n.as_str()), n.as_str()));
    let mut out =
        String::from("| Parameter | Type | Required | Description |\n|---|---|---|---|\n");
    for n in names {
        let prop = &props[n.as_str()];
        let desc = prop
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .replace('|', "\\|")
            .replace('\n', " ");
        out.push_str(&format!(
            "| `{n}` | {} | {} | {desc} |\n",
            param_type(prop).replace('|', "\\|"),
            if required.contains(&n.as_str()) {
                "yes"
            } else {
                ""
            },
        ));
    }
    out
}

/// The schema's `allOf` conditions in words: "When `action` is `x`: requires
/// `a` and `b`".
/// `a`, `a and b`, `a, b and c`.
fn list(items: &[String], conj: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} {conj} {last}", init.join(", ")),
    }
}

fn render_conditions(schema: Option<&Value>) -> String {
    let Some(all_of) = schema
        .and_then(|s| s.get("allOf"))
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|n| format!("`{n}`"))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut lines = Vec::new();
    for cond in all_of {
        let Some(props) = cond
            .get("if")
            .and_then(|i| i.get("properties"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        let when: Vec<String> = props
            .iter()
            .map(|(key, test)| {
                let values: Vec<String> = test
                    .get("const")
                    .map(|c| vec![c.clone()])
                    .or_else(|| test.get("enum").and_then(Value::as_array).cloned())
                    .unwrap_or_default()
                    .iter()
                    .map(|v| format!("`{}`", v.as_str().unwrap_or_default()))
                    .collect();
                format!("`{key}` is {}", list(&values, "or"))
            })
            .collect();
        let Some(then) = cond.get("then") else {
            continue;
        };
        let mut needs = Vec::new();
        if let Some(req) = then.get("required") {
            needs.push(format!("requires {}", list(&names(req), "and")));
        }
        if let Some(any) = then.get("anyOf").and_then(Value::as_array) {
            let alts: Vec<String> = any
                .iter()
                .filter_map(|a| a.get("required"))
                .map(|r| list(&names(r), "and"))
                .collect();
            needs.push(format!("requires {}", alts.join(" or ")));
        }
        if !needs.is_empty() {
            lines.push(format!(
                "- When {}: {}.",
                when.join(" and "),
                needs.join("; ")
            ));
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Config keys
// ---------------------------------------------------------------------------

/// Markdown reference for every recognized `config.toml` key, rendered from
/// the `Config` schema (types, defaults, allowed values, env overrides).
pub fn config_keys_markdown() -> String {
    let schema = ConfigSchema::generate();

    let mut out = String::new();
    out.push_str("# Appendix — Configuration Keys (generated)\n\n");
    out.push_str(DO_NOT_EDIT);
    out.push_str("\n\n");
    out.push_str("Source of truth: `rust/src/core/config/schema/`.\n\n");
    out.push_str(
        "sobre reads one file, `~/.config/sobre/config.toml` (the directory \
         follows `$XDG_CONFIG_HOME`, or `$SOBRE_CONFIG_DIR` when set). \
         Below is every recognized key with its type and default.\n\n\
         - **Reading.** The file is read again when it changes. A file that \
         does not parse is ignored whole: every key takes its default, and a \
         warning goes to stderr once. An unknown key is ignored; `sobre doctor` \
         names it.\n\
         - **`sobre config set <key> <value>`.** Booleans take `true`/`false` \
         (also `yes`/`no`, `1`/`0`); a list takes comma-separated \
         values. The value is checked before the file is written, and the \
         file keeps its comments and key order. `default` or an empty value \
         removes the key, so its default applies.\n\
         - **`sobre config init`** writes the file with every key documented, \
         keeping an existing one as `config.toml.sobre.bak`.\n\n",
    );

    // `root` first (top-level keys), then named sections alphabetically.
    if let Some(root) = schema.sections.get("root") {
        out.push_str("## Top-level keys\n\n");
        if !root.description.trim().is_empty() {
            out.push_str(&format!("{}\n\n", root.description.trim()));
        }
        out.push_str(&render_section_keys(root));
    }

    for (name, section) in &schema.sections {
        if name == "root" {
            continue;
        }
        out.push_str(&format!("## `[{name}]`\n\n"));
        if !section.description.trim().is_empty() {
            out.push_str(&format!("{}\n\n", section.description.trim()));
        }
        let keys = render_section_keys(section);
        if keys.is_empty() {
            out.push_str("_No sub-keys (presence of the section toggles the feature)._\n\n");
        } else {
            out.push_str(&keys);
        }
    }
    out.push_str("## Built-in passthrough commands\n\n");
    out.push_str(
        "Besides `passthrough_commands`, these commands' output is never \
         compressed: interactive tools, dev servers, watchers, auth flows. An \
         entry matches the command, or a word of a pipeline; an entry starting \
         with `--` matches a flag anywhere.\n\n",
    );
    let (builtin, runners, keywords) = crate::shell::compress::builtin_passthrough();
    let quoted = |items: &[&str]| {
        items
            .iter()
            .map(|i| format!("`{}`", i.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    out.push_str(&format!("{}\n\n", quoted(builtin)));
    out.push_str(&format!(
        "Also a script run through {} whose name contains {}.\n",
        quoted(runners),
        quoted(keywords)
    ));
    out
}

fn render_section_keys(section: &crate::core::config::schema::SectionSchema) -> String {
    let mut out = String::new();
    for (key, ks) in &section.keys {
        let ty = &ks.ty;
        let default = value_to_inline(&ks.default);
        let desc = ks.description.trim();
        out.push_str(&format!("- `{key}` ({ty}, default `{default}`) — {desc}\n"));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Compact inline rendering of a JSON default value for docs.
fn value_to_inline(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::String(s) if s.is_empty() => "\"\"".to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) if a.is_empty() => "[]".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tools_doc_lists_every_registered_tool() {
        let md = mcp_tools_markdown();
        let count = crate::server::registry::tool_count();
        // Every registered tool gets its own `## ` section heading.
        let headings = md.matches("\n## `").count();
        assert!(
            headings >= count,
            "expected at least one heading per tool: {headings} headings for {count} tools"
        );
        // Spot-check a couple of always-present tools.
        assert!(
            md.contains("## `sobre_read`"),
            "sobre_read must be documented"
        );
        assert!(
            md.contains("## `sobre_shell`"),
            "sobre_shell must be documented"
        );
    }

    #[test]
    fn config_keys_doc_covers_all_known_keys() {
        let md = config_keys_markdown();
        let schema = ConfigSchema::generate();
        for key in schema.sections.values().flat_map(|s| s.keys.keys()) {
            assert!(
                md.contains(&format!("`{key}`")),
                "config key `{key}` missing from generated doc"
            );
        }
    }

    #[test]
    fn content_matches_ignores_line_endings() {
        // A CRLF checkout (Windows) must still match LF-generated content.
        assert!(content_matches("a\r\nb\r\n", "a\nb\n"));
        assert!(content_matches("a\nb\n", "a\nb\n"));
        // Real content differences still fail.
        assert!(!content_matches("a\nb\n", "a\nc\n"));
    }

    #[test]
    fn generated_docs_are_nonempty_and_named() {
        let docs = generated_docs();
        assert_eq!(docs.len(), 4);
        for (name, body) in docs {
            assert!(
                std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
                "{name} should be a .md file"
            );
            assert!(body.len() > 100, "{name} should not be trivial");
            assert!(body.contains("GENERATED FILE"), "{name} needs the banner");
        }
    }
}
