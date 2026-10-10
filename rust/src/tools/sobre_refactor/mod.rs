use lsp_types::{Location, Position};
use serde_json::Value;

use crate::lsp::client::uri_to_file_path;

pub fn handle(args: &Value, project_root: &str, abs_path: &str) -> String {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("references");

    if matches!(
        action,
        "replace_symbol_body" | "insert_before_symbol" | "insert_after_symbol"
    ) {
        return handle_symbol_edit(action, args, project_root);
    }

    if !matches!(
        action,
        "rename" | "references" | "definition" | "implementations"
    ) {
        return format!(
            "ERROR: Unknown action '{action}'. Available: rename (preview), references, \
             definition, implementations, replace_symbol_body, insert_before_symbol, \
             insert_after_symbol. For a file's symbols use sobre_outline."
        );
    }

    let line = args.get("line").and_then(Value::as_u64).unwrap_or(1) as u32;
    let column = args.get("column").and_then(Value::as_u64).unwrap_or(0) as u32;

    let uri = match crate::lsp::router::open_file(abs_path, project_root) {
        Ok(u) => u,
        Err(e) => return format!("ERROR: {e}"),
    };

    let position = Position::new(line.saturating_sub(1), column);

    match action {
        "rename" => handle_rename(args, abs_path, project_root, &uri, position),
        "references" => handle_references(abs_path, project_root, &uri, position),
        "definition" => handle_definition(abs_path, project_root, &uri, position),
        _ => handle_implementations(abs_path, project_root, &uri, position),
    }
}

fn handle_rename(
    args: &Value,
    file_path: &str,
    project_root: &str,
    uri: &lsp_types::Uri,
    position: Position,
) -> String {
    let Some(new_name) = args.get("new_name").and_then(Value::as_str) else {
        return "ERROR: 'new_name' parameter is required for rename.".to_string();
    };

    let result = crate::lsp::router::with_backend(file_path, project_root, |backend, _| {
        backend.rename(uri, position, new_name)
    });

    match result {
        Ok(Some(edit)) => format_workspace_edit(&edit, project_root),
        Ok(None) => "No rename edits returned by language server.".to_string(),
        Err(e) => format!("ERROR: {e}"),
    }
}

fn handle_references(
    file_path: &str,
    project_root: &str,
    uri: &lsp_types::Uri,
    position: Position,
) -> String {
    let result = crate::lsp::router::with_backend(file_path, project_root, |backend, _| {
        backend.references(uri, position)
    });

    match result {
        Ok(locations) => format_locations(&locations, project_root),
        Err(e) => format!("ERROR: {e}"),
    }
}

fn handle_definition(
    file_path: &str,
    project_root: &str,
    uri: &lsp_types::Uri,
    position: Position,
) -> String {
    let result = crate::lsp::router::with_backend(file_path, project_root, |backend, _| {
        backend.definition(uri, position)
    });

    match result {
        Ok(resp) => {
            let locations = match resp {
                lsp_types::GotoDefinitionResponse::Scalar(loc) => vec![loc],
                lsp_types::GotoDefinitionResponse::Array(locs) => locs,
                lsp_types::GotoDefinitionResponse::Link(links) => links
                    .into_iter()
                    .map(|l| Location {
                        uri: l.target_uri,
                        range: l.target_selection_range,
                    })
                    .collect(),
            };
            format_locations(&locations, project_root)
        }
        Err(e) => format!("ERROR: {e}"),
    }
}

fn handle_implementations(
    file_path: &str,
    project_root: &str,
    uri: &lsp_types::Uri,
    position: Position,
) -> String {
    let result = crate::lsp::router::with_backend(file_path, project_root, |backend, _| {
        backend.implementations(uri, position)
    });

    match result {
        Ok(locations) => format_locations(&locations, project_root),
        Err(e) => format!("ERROR: {e}"),
    }
}

/// A resolved symbol location (project-relative path + 1-based inclusive line span).
#[derive(Debug)]
pub(crate) struct Resolved {
    pub rel_path: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Apply a resolved edit through the headless range writer.
fn apply_symbol_edit(
    action: &str,
    edit: &crate::lsp::backend::RangeEdit,
) -> Result<crate::lsp::backend::EditResult, String> {
    use crate::lsp::backend::LspBackend;
    let mut backend = crate::lsp::edit_apply::HeadlessBackend;
    match action {
        "replace_symbol_body" => backend.replace_symbol_body(edit),
        "insert_before_symbol" => backend.insert_before_symbol(edit),
        "insert_after_symbol" => backend.insert_after_symbol(edit),
        other => Err(format!("INTERNAL: not an edit action: {other}")),
    }
}

/// Leading whitespace of the 1-based `line` in `content` (anchor indentation).
fn anchor_indent(content: &str, line: usize) -> String {
    content
        .lines()
        .nth(line.saturating_sub(1))
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect())
        .unwrap_or_default()
}

/// Prefix `indent` to the first line of `text` iff that line has no leading
/// whitespace of its own.
fn reindent_first_line(text: &str, indent: &str) -> String {
    if text.starts_with(' ') || text.starts_with('\t') || indent.is_empty() {
        return text.to_string();
    }
    format!("{indent}{text}")
}

/// True if symbol `name` denotes a container for type `ancestor`: the bare type
/// itself (struct/enum/inherent `impl Type`) — exact match — or a trait impl,
/// whose indexed name is `<Trait> for <Type>` (see the round-trip note in
/// graph_provider.rs). Generic args on the impl target (`… for Type<T>`) are
/// stripped so `Type/method` still resolves. Language-agnostic: non-Rust
/// container names never contain `" for "`, so only the exact branch applies.
fn container_matches_ancestor(name: &str, ancestor: &str) -> bool {
    if name == ancestor {
        return true;
    }
    match name.rsplit_once(" for ") {
        Some((_, target)) => target.split('<').next().unwrap_or(target).trim() == ancestor,
        None => false,
    }
}

/// Resolve a `name_path` (`Class/method` or bare `name`) to a single symbol,
/// with its line span read from the file as it is now. The index only says
/// which files define the name (`file_scope`, a project-relative path, narrows
/// that to one file); each candidate file is parsed again, so an edit never
/// lands on lines the index recorded before the file changed. A qualified
/// path is disambiguated by containment: the ancestor's span holds the leaf's.
pub(crate) fn resolve_name_path_scoped(
    name_path: &str,
    project_root: &str,
    file_scope: Option<&str>,
) -> Result<Resolved, String> {
    let segments: Vec<&str> = name_path.split('/').filter(|s| !s.is_empty()).collect();
    let leaf = *segments
        .last()
        .ok_or_else(|| "NO_SYMBOL: empty name_path".to_string())?;
    let ancestor = segments.len().checked_sub(2).map(|i| segments[i]);

    let files: Vec<String> = if let Some(f) = file_scope {
        vec![f.to_string()]
    } else {
        let open = crate::core::graph_provider::open_or_build(project_root)
            .ok_or_else(|| "NO_SYMBOL: no symbol index available".to_string())?;
        let mut files: Vec<String> = open
            .find_symbols(leaf, None, None)
            .into_iter()
            .filter(|s| s.name == leaf)
            .map(|s| s.file)
            .collect();
        files.sort();
        files.dedup();
        files
    };

    let mut found: Vec<(String, String, usize, usize)> = Vec::new();
    for rel in files {
        let abs = std::path::Path::new(project_root).join(&rel);
        let Ok(content) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("");
        let spans = crate::core::graph_index::symbol_spans(&content, ext);
        for (name, start, end) in &spans {
            if name != leaf {
                continue;
            }
            let contained = ancestor.is_none_or(|anc| {
                spans.iter().any(|(parent, p_start, p_end)| {
                    container_matches_ancestor(parent, anc) && p_start <= start && end <= p_end
                })
            });
            if contained {
                found.push((rel.clone(), name.clone(), *start, *end));
            }
        }
    }

    match found.len() {
        0 => Err(format!(
            "NO_SYMBOL: '{name_path}' did not resolve to any symbol{}",
            if file_scope.is_some() {
                " in that file"
            } else {
                " (a symbol added since the last index needs 'path')"
            }
        )),
        1 => {
            let (rel_path, _, start_line, end_line) = found.remove(0);
            Ok(Resolved {
                rel_path,
                start_line,
                end_line,
            })
        }
        n => {
            let mut msg =
                format!("AMBIGUOUS_SYMBOL: '{name_path}' matches {n} symbols; qualify it:\n");
            for (file, name, start, end) in found.iter().take(10) {
                msg.push_str(&format!("  {file}:{name} (L{start}-{end})\n"));
            }
            Err(msg)
        }
    }
}

/// The innermost symbol of `content` whose span holds `line` (1-based), with
/// its current span.
fn symbol_at_line(content: &str, ext: &str, line: usize) -> Option<(usize, usize)> {
    crate::core::graph_index::symbol_spans(content, ext)
        .into_iter()
        .filter(|(_, start, end)| *start <= line && line <= *end)
        .min_by_key(|(_, start, end)| end - start)
        .map(|(_, start, end)| (start, end))
}

fn handle_symbol_edit(action: &str, args: &Value, project_root: &str) -> String {
    let explicit_path = args.get("path").and_then(Value::as_str);

    let (rel_path, start_line, end_line) = if let Some(np) =
        args.get("name_path").and_then(Value::as_str)
    {
        // With `path`, the name is looked up in that file only: a common name
        // defined elsewhere is neither ambiguous nor a wrong target.
        let scope =
            explicit_path.map(|p| crate::core::index_paths::graph_relative_key(p, project_root));
        match resolve_name_path_scoped(np, project_root, scope.as_deref()) {
            Ok(r) => (r.rel_path, r.start_line, r.end_line),
            Err(e) => return format!("ERROR: {e}"),
        }
    } else {
        let Some(path) = explicit_path else {
            return "ERROR: provide 'name_path' or 'path'+'line' for symbol edits.".to_string();
        };
        let line = args.get("line").and_then(Value::as_u64).unwrap_or(0) as usize;
        if line == 0 {
            return "ERROR: 'line' is required (1-based) with 'path' and no 'name_path'."
                .to_string();
        }
        // The symbol around `line`, as the file is now.
        let abs = match crate::core::path_resolve::resolve_tool_path(Some(project_root), None, path)
        {
            Ok(p) => p,
            Err(e) => return format!("ERROR: {e}"),
        };
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => return format!("ERROR: FILE_NOT_FOUND: {abs}: {e}"),
        };
        let ext = std::path::Path::new(&abs)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("");
        let Some((start, end)) = symbol_at_line(&content, ext, line) else {
            return format!("ERROR: NO_SYMBOL: no symbol spans line {line} of {path}");
        };
        (path.to_string(), start, end)
    };

    // 2) Resolve the path against the project root.
    let abs_path =
        match crate::core::path_resolve::resolve_tool_path(Some(project_root), None, &rel_path) {
            Ok(p) => p,
            Err(e) => return format!("ERROR: {e}"),
        };

    let content = match std::fs::read_to_string(&abs_path) {
        Ok(c) => c,
        Err(e) => return format!("ERROR: FILE_NOT_FOUND: {abs_path}: {e}"),
    };

    // 3) Build the canonical range + final wire text per action.
    let expected_hash = args
        .get("expected_hash")
        .and_then(Value::as_str)
        .map(String::from);
    let (range, text) = match action {
        "replace_symbol_body" => {
            let Some(new_text) = args.get("new_text").and_then(Value::as_str) else {
                return "ERROR: 'new_text' is required for replace_symbol_body.".to_string();
            };
            let end_col = content
                .lines()
                .nth(end_line.saturating_sub(1))
                .map_or(0, str::len) as u32;
            (
                crate::lsp::backend::TextRange0Based {
                    start_line: (start_line - 1) as u32,
                    start_char: 0,
                    end_line: (end_line - 1) as u32,
                    end_char: end_col,
                },
                new_text.to_string(),
            )
        }
        "insert_before_symbol" | "insert_after_symbol" => {
            let Some(t) = args.get("text").and_then(Value::as_str) else {
                return format!("ERROR: 'text' is required for {action}.");
            };
            let indent = anchor_indent(&content, start_line);
            let final_text = format!("{}\n", reindent_first_line(t, &indent));
            let insert_line = if action == "insert_before_symbol" {
                (start_line - 1) as u32
            } else {
                end_line as u32
            };
            (
                crate::lsp::backend::TextRange0Based {
                    start_line: insert_line,
                    start_char: 0,
                    end_line: insert_line,
                    end_char: 0,
                },
                final_text,
            )
        }
        other => return format!("ERROR: INTERNAL: not an edit action: {other}"),
    };

    // CONFLICT guard: verify expected_hash (BLAKE3 hex of the range) against
    // the file as it is now, so a dry run reports the conflict too. The
    // writer re-checks it under the file lock.
    if let Some(exp) = &expected_hash {
        let s =
            match crate::lsp::edit_apply::offset_of(&content, range.start_line, range.start_char) {
                Ok(o) => o,
                Err(e) => return format!("ERROR: {e}"),
            };
        let e = match crate::lsp::edit_apply::offset_of(&content, range.end_line, range.end_char) {
            Ok(o) => o,
            Err(e) => return format!("ERROR: {e}"),
        };
        if e < s {
            return "ERROR: POSITION_OUT_OF_RANGE: end before start".to_string();
        }
        let actual = crate::core::hasher::hash_hex(&content.as_bytes()[s..e]);
        if *exp != actual {
            return format!(
                "ERROR: CONFLICT: range hash mismatch (expected={exp}, actual={actual})"
            );
        }
    }

    let edit = crate::lsp::backend::RangeEdit {
        abs_path,
        rel_path,
        range,
        text,
        expected_hash,
    };

    // 4) The new content, for the syntax report and the dry run.
    let ext = std::path::Path::new(&edit.abs_path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("");
    let start_off = crate::lsp::edit_apply::offset_of(&content, range.start_line, range.start_char)
        .unwrap_or(0);
    let end_off = crate::lsp::edit_apply::offset_of(&content, range.end_line, range.end_char)
        .unwrap_or(content.len());
    let mut hypothetical =
        String::with_capacity(content.len() - (end_off - start_off) + edit.text.len());
    hypothetical.push_str(&content[..start_off]);
    hypothetical.push_str(&edit.text);
    hypothetical.push_str(&content[end_off..]);
    // A syntax error the edit introduces is reported; the edit is written
    // either way.
    let syntax_notice = crate::core::syntax_validate::syntax_notice(ext, &content, &hypothetical);

    if args.get("dry_run").and_then(Value::as_bool) == Some(true) {
        let mut out = crate::tools::impl_edit::dry_run_text(
            std::path::Path::new(&edit.abs_path),
            &content,
            &hypothetical,
        );
        crate::tools::impl_edit::append_notice(&mut out, syntax_notice);
        return out;
    }

    // 5) Write the range and format the result.
    match apply_symbol_edit(action, &edit) {
        Ok(res) => {
            let mut out = format_edit_result(action, &edit.abs_path, &res);
            crate::tools::impl_edit::append_notice(&mut out, syntax_notice);
            out
        }
        Err(e) => format!("ERROR: {e}"),
    }
}

fn format_edit_result(
    action: &str,
    abs_path: &str,
    res: &crate::lsp::backend::EditResult,
) -> String {
    if !res.applied {
        return format!("{action}: not applied.");
    }
    let r = res.new_range;
    let body = if res.diff.is_empty() {
        res.edited_text.clone()
    } else {
        res.diff.clone()
    };
    // Include absolute path so callers can verify the write target
    format!(
        "{action} applied {abs_path} (L{}:{}-L{}:{}):\n{}",
        r.start_line + 1,
        r.start_char,
        r.end_line + 1,
        r.end_char,
        body
    )
}

fn format_locations(locations: &[Location], project_root: &str) -> String {
    if locations.is_empty() {
        return "No results found.".to_string();
    }

    let mut out = format!("{} location(s):\n", locations.len());
    for loc in locations {
        let path = uri_to_file_path(&loc.uri).map_or_else(
            || loc.uri.as_str().to_string(),
            |p| {
                p.strip_prefix(project_root)
                    .map(|s| s.strip_prefix('/').unwrap_or(s).to_string())
                    .unwrap_or(p)
            },
        );

        let line = loc.range.start.line + 1;
        let col = loc.range.start.character;
        out.push_str(&format!("  {path}:{line}:{col}\n"));
    }
    out
}

fn format_workspace_edit(edit: &lsp_types::WorkspaceEdit, project_root: &str) -> String {
    let mut out = String::from("Rename preview — nothing was written:\n");
    let mut file_count = 0;
    let mut edit_count = 0;

    if let Some(ref changes) = edit.changes {
        for (uri, edits) in changes {
            let path = uri_to_file_path(uri).map_or_else(
                || uri.as_str().to_string(),
                |p| {
                    p.strip_prefix(project_root)
                        .map(|s| s.strip_prefix('/').unwrap_or(s).to_string())
                        .unwrap_or(p)
                },
            );

            file_count += 1;
            out.push_str(&format!("  {path}: {} edit(s)\n", edits.len()));
            for e in edits {
                edit_count += 1;
                let line = e.range.start.line + 1;
                out.push_str(&format!("    L{line}: -> \"{}\"\n", e.new_text));
            }
        }
    }

    if let Some(ref doc_changes) = edit.document_changes {
        match doc_changes {
            lsp_types::DocumentChanges::Edits(edits) => {
                for text_edit in edits {
                    let path = uri_to_file_path(&text_edit.text_document.uri)
                        .unwrap_or_else(|| text_edit.text_document.uri.as_str().to_string());
                    file_count += 1;
                    let edits_len = text_edit.edits.len();
                    edit_count += edits_len;
                    out.push_str(&format!("  {path}: {edits_len} edit(s)\n"));
                }
            }
            lsp_types::DocumentChanges::Operations(ops) => {
                for op in ops {
                    if let lsp_types::DocumentChangeOperation::Edit(text_edit) = op {
                        let path = uri_to_file_path(&text_edit.text_document.uri)
                            .unwrap_or_else(|| text_edit.text_document.uri.as_str().to_string());
                        file_count += 1;
                        let edits_len = text_edit.edits.len();
                        edit_count += edits_len;
                        out.push_str(&format!("  {path}: {edits_len} edit(s)\n"));
                    }
                }
            }
        }
    }

    out.push_str(&format!(
        "\nTotal: {edit_count} edit(s) across {file_count} file(s). Apply them with sobre_patch."
    ));
    out
}

#[cfg(test)]
mod tests;
