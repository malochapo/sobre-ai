//! Parity guard: every `ctx_*` tool name shipped in an agent template must map
//! to a real registered MCP tool. A static template has no compile-time tie to
//! the registry, so this locks every shipped template to the canonical tool
//! surface and future renames cannot drift unnoticed (#548).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lean_ctx::server::registry::build_registry;

fn templates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/templates")
}

/// Extract distinct `ctx_<name>` identifiers from `text`, where `<name>` is one
/// or more ASCII alphanumerics / underscores. Bare `ctx_` and globs like
/// `ctx_*` carry no tool name and are skipped.
fn ctx_tool_tokens(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut tokens = BTreeSet::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if &bytes[i..i + 4] == b"ctx_" {
            let mut end = i + 4;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > i + 4 {
                tokens.insert(text[i..end].to_string());
            }
            i = end.max(i + 4);
        } else {
            i += 1;
        }
    }
    tokens
}

/// Shipped templates the agent installers write verbatim (`.md` / `.txt`).
fn template_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(templates_dir())
        .expect("templates dir is readable")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("md" | "txt")))
        .collect();
    files.sort();
    files
}

#[test]
fn templates_only_reference_registered_ctx_tools() {
    let registry = build_registry();
    let mut violations: Vec<String> = Vec::new();

    let files = template_files();
    assert!(!files.is_empty(), "expected at least one shipped template");

    for path in files {
        let text = std::fs::read_to_string(&path).expect("template is readable");
        for token in ctx_tool_tokens(&text) {
            if !registry.contains(&token) {
                violations.push(format!(
                    "{}: `{token}` is not a registered MCP tool",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "shipped agent templates reference unknown/renamed ctx_* tools:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn token_extractor_ignores_globs_and_bare_prefix() {
    assert!(ctx_tool_tokens("use ctx_* tools and ctx_ alone").is_empty());
    let tokens = ctx_tool_tokens("`ctx_read`/ctx_shell, ctx_search(pattern)");
    assert!(tokens.contains("ctx_read"));
    assert!(tokens.contains("ctx_shell"));
    assert!(tokens.contains("ctx_search"));
}
