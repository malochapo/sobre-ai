use std::collections::HashMap;
use std::path::Path;

use crate::core::graph_index;
use crate::core::graph_provider::{self, GraphProvider};
use crate::core::index_paths;
use crate::core::tokens::count_tokens;

#[allow(clippy::too_many_arguments)]
pub fn handle(
    action: &str,
    path: Option<&str>,
    root: &str,
    cache: &mut crate::core::cache::SessionCache,
    crp_mode: crate::tools::CrpMode,
    depth: Option<usize>,
    kind: Option<&str>,
    to: Option<&str>,
    format: Option<&str>,
) -> String {
    match action {
        "build" => handle_build(root),
        "related" => handle_related(path, root),
        "symbol" => handle_symbol(path, root, cache, crp_mode),
        "status" => handle_status(root),
        "diagram" => crate::tools::ctx_graph_diagram::handle(path, depth, kind, root),
        "neighbors" => crate::tools::ctx_graph_primitives::neighbors(path, root, depth, format),
        "path" => crate::tools::ctx_graph_primitives::shortest_path(path, to, root, format),
        _ => "Unknown action. Use: build, status, related, symbol, impact, changes, neighbors, \
              path, diagram, callers, callees, trace"
            .to_string(),
    }
}

fn handle_build(root: &str) -> String {
    let index = graph_index::scan(root);

    let mut by_lang: HashMap<&str, (usize, usize)> = HashMap::new();
    for entry in index.files.values() {
        let e = by_lang.entry(&entry.language).or_insert((0, 0));
        e.0 += 1;
        e.1 += entry.token_count;
    }

    let mut result = Vec::new();
    result.push(format!(
        "Project Graph: {} files, {} symbols, {} edges",
        index.file_count(),
        index.symbol_count(),
        index.edge_count()
    ));

    let mut langs: Vec<_> = by_lang.iter().collect();
    langs.sort_by_key(|(_, v)| std::cmp::Reverse(v.1));
    result.push("\nLanguages:".to_string());
    for (lang, (count, tokens)) in &langs {
        result.push(format!("  {lang}: {count} files, {tokens} tok"));
    }

    let mut import_counts: HashMap<&str, usize> = HashMap::new();
    for edge in &index.edges {
        if edge.kind == "import" {
            *import_counts.entry(&edge.to).or_insert(0) += 1;
        }
    }
    let mut hotspots: Vec<_> = import_counts.iter().collect();
    hotspots.sort_by_key(|x| std::cmp::Reverse(*x.1));

    if !hotspots.is_empty() {
        result.push(format!("\nMost imported ({}):", hotspots.len().min(10)));
        for (module, count) in hotspots.iter().take(10) {
            result.push(format!("  {module}: imported by {count} files"));
        }
    }

    if let Some(dir) = GraphProvider::index_dir(root) {
        result.push(format!(
            "\nIndex saved: {}",
            crate::core::protocol::shorten_path(&dir.to_string_lossy())
        ));
    }

    let output = result.join("\n");
    let tokens = count_tokens(&output);
    format!("{output}\n[ctx_graph build: {tokens} tok]")
}

fn handle_related(path: Option<&str>, root: &str) -> String {
    let Some(target) = path else {
        return "ERROR: path is required for 'related' action".to_string();
    };

    let Some(open) = graph_provider::open_or_build(root) else {
        return "No graph index found. Run ctx_graph with action='build' first.".to_string();
    };

    let rel_target = index_paths::graph_relative_key(target, root);

    let related = open.related(&rel_target, 2);
    if related.is_empty() {
        return format!(
            "No related files found for {}",
            crate::core::protocol::shorten_path(target)
        );
    }

    let mut result = format!(
        "Files related to {} ({}):\n",
        crate::core::protocol::shorten_path(target),
        related.len()
    );
    for r in &related {
        result.push_str(&format!("  {}\n", crate::core::protocol::shorten_path(r)));
    }

    let tokens = count_tokens(&result);
    format!("{result}[ctx_graph related: {tokens} tok]")
}

fn handle_symbol(
    path: Option<&str>,
    root: &str,
    cache: &mut crate::core::cache::SessionCache,
    crp_mode: crate::tools::CrpMode,
) -> String {
    let Some(spec) = path else {
        return "ERROR: a symbol is required for 'symbol' action (a bare <symbol> name, or <file>::<symbol>)".to_string();
    };

    let Some(open) = graph_provider::open_or_build(root) else {
        return "No graph index found. Run ctx_graph with action='build' first.".to_string();
    };

    // Bare symbol name (no `::`): resolve against the symbol table so GDScript
    // (and every other language) symbols are reachable without a file qualifier
    // (#314).
    let Some((file_part, symbol_name)) = spec.split_once("::") else {
        return resolve_bare_symbol(&open, spec, root, cache, crp_mode);
    };

    let rel_file = index_paths::graph_relative_key(file_part, root);

    let key = format!("{rel_file}::{symbol_name}");
    let Some(symbol) = open.get_symbol(&key) else {
        let available = open.find_symbols(symbol_name, Some(&rel_file), None);
        if available.is_empty() {
            return format!(
                "Symbol '{symbol_name}' not found in {rel_file}. Run ctx_graph action='build' to update the index."
            );
        }
        let names: Vec<String> = available
            .iter()
            .take(10)
            .map(|s| format!("{}::{}", s.file, s.name))
            .collect();
        return format!(
            "Symbol '{symbol_name}' not found in {rel_file}.\nAvailable symbols:\n  {}",
            names.join("\n  ")
        );
    };

    let abs_path = if Path::new(file_part).is_absolute() {
        file_part.to_string()
    } else {
        Path::new(root)
            .join(rel_file.trim_start_matches(['/', '\\']))
            .to_string_lossy()
            .to_string()
    };

    render_symbol_snippet(&symbol, &abs_path, &rel_file, cache, crp_mode)
}

/// Resolve a bare symbol name (no `<file>::` qualifier) against the symbol table.
/// One unambiguous hit renders the snippet; otherwise the candidates are listed
/// so the caller can disambiguate with `<file>::<symbol>` (#314).
fn resolve_bare_symbol(
    provider: &GraphProvider,
    name: &str,
    root: &str,
    cache: &mut crate::core::cache::SessionCache,
    crp_mode: crate::tools::CrpMode,
) -> String {
    let matches = provider.find_symbols(name, None, None);
    if matches.is_empty() {
        return format!(
            "Symbol '{name}' not found. Run ctx_graph action='build' to update the index."
        );
    }

    let name_lower = name.to_lowercase();
    let exact: Vec<&graph_provider::SymbolInfo> = matches
        .iter()
        .filter(|s| s.name.to_lowercase() == name_lower)
        .collect();

    if let [only] = exact.as_slice() {
        let abs_path = Path::new(root)
            .join(only.file.trim_start_matches(['/', '\\']))
            .to_string_lossy()
            .to_string();
        return render_symbol_snippet(only, &abs_path, &only.file, cache, crp_mode);
    }

    // Several identically-named symbols, or only substring hits → list them.
    let shortlist: Vec<&graph_provider::SymbolInfo> = if exact.is_empty() {
        matches.iter().collect()
    } else {
        exact
    };
    let mut lines = vec![format!(
        "Symbol '{name}' matches {} entries — pick one with `<file>::{name}`:",
        shortlist.len()
    )];
    for s in shortlist.iter().take(15) {
        lines.push(format!(
            "  {}::{} ({}, {}:{})",
            crate::core::protocol::shorten_path(&s.file),
            s.name,
            s.kind,
            s.start_line,
            s.end_line
        ));
    }
    lines.join("\n")
}

/// Render a symbol's source snippet with the standard token-savings footer.
/// Shared by the qualified (`<file>::<symbol>`) and bare-name resolution paths.
fn render_symbol_snippet(
    symbol: &graph_provider::SymbolInfo,
    abs_path: &str,
    rel_display: &str,
    cache: &mut crate::core::cache::SessionCache,
    crp_mode: crate::tools::CrpMode,
) -> String {
    let content = match std::fs::read_to_string(abs_path) {
        Ok(c) => c,
        Err(e) => return format!("Cannot read {abs_path}: {e}"),
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = symbol.start_line.saturating_sub(1);
    let end = symbol.end_line.min(lines.len());

    if start >= lines.len() {
        return crate::tools::ctx_read::handle(cache, abs_path, "full", crp_mode);
    }

    let mut result = format!(
        "{}::{} ({}:{}-{})\n",
        crate::core::protocol::shorten_path(rel_display),
        symbol.name,
        symbol.kind,
        symbol.start_line,
        symbol.end_line
    );

    for (i, line) in lines[start..end].iter().enumerate() {
        result.push_str(&format!("{:>4}|{}\n", start + i + 1, line));
    }

    let tokens = count_tokens(&result);
    let full_tokens = count_tokens(&content);
    let saved = full_tokens.saturating_sub(tokens);
    let pct = if full_tokens > 0 {
        (saved as f64 / full_tokens as f64 * 100.0).round() as usize
    } else {
        0
    };

    format!("{result}[ctx_graph symbol: {tokens} tok (full file: {full_tokens} tok, -{pct}%)]")
}

fn handle_status(root: &str) -> String {
    let Some(open) = graph_provider::open_best_effort(root) else {
        return "No graph index. Run ctx_graph action='build' to create one.".to_string();
    };
    let gp = &open;

    let file_paths = gp.file_paths();
    let mut by_lang: HashMap<String, usize> = HashMap::new();
    let mut total_tokens = 0usize;
    for path in &file_paths {
        if let Some(entry) = gp.get_file_entry(path) {
            *by_lang.entry(entry.language).or_insert(0) += 1;
            total_tokens += entry.token_count;
        }
    }

    let mut langs: Vec<_> = by_lang.iter().collect();
    langs.sort_by_key(|item| std::cmp::Reverse(*item.1));
    let lang_summary: String = langs
        .iter()
        .take(5)
        .map(|(l, c)| format!("{l}:{c}"))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "Graph: {} files, {} symbols, {} edges | {} tok total\nLast scan: {} ({})\nLanguages: {lang_summary}\nStored: {}",
        gp.file_count(),
        gp.symbol_count(),
        gp.edge_count().unwrap_or(0),
        total_tokens,
        gp.last_scan(),
        crate::tools::ctx_graph_impact::freshness(root),
        GraphProvider::index_dir(root)
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default()
    )
}

/// End-to-end GDScript graph coverage on a real Godot fixture (#314): four `.gd`
/// scripts with `_ready` definitions and `res://` import edges, exercised through
/// the public graph actions (context / impact / bare symbol).
#[cfg(test)]
mod gdscript_p0_tests {
    use super::*;

    fn write(root: &std::path::Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// Minimal Godot project: `Player`/`Enemy` extend `Base`, `main` preloads
    /// `Player`; every script defines `_ready`. Returns (tempdir guard, root).
    fn godot_fixture() -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

        let proj = tmp.path().join("game");
        std::fs::create_dir_all(&proj).unwrap();
        write(
            &proj,
            "project.godot",
            "[application]\nconfig/name=\"Fixture\"\n",
        );
        write(
            &proj,
            "actors/Base.gd",
            "extends Node\n\nfunc _ready():\n\tpass\n",
        );
        write(
            &proj,
            "actors/Player.gd",
            "extends \"res://actors/Base.gd\"\n\nfunc _ready():\n\tprint(\"player\")\n",
        );
        write(
            &proj,
            "actors/Enemy.gd",
            "extends \"res://actors/Base.gd\"\n\nfunc _ready():\n\tprint(\"enemy\")\n",
        );
        write(
            &proj,
            "main.gd",
            "const Player = preload(\"res://actors/Player.gd\")\n\nfunc _ready():\n\tprint(\"main\")\n",
        );

        (tmp, proj.to_string_lossy().to_string())
    }

    #[test]
    fn impact_lists_gdscript_dependents() {
        let _lock = crate::core::data_dir::test_env_lock();
        let (_tmp, root) = godot_fixture();
        let _ = handle_build(&root);
        let out = crate::tools::ctx_graph_impact::impact(Some("actors/Base.gd"), &root, None, None);
        assert!(
            out.contains("Player.gd"),
            "Base.gd dependents should include Player: {out}"
        );
        assert!(
            out.contains("Enemy.gd"),
            "Base.gd dependents should include Enemy: {out}"
        );
        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn bare_symbol_resolves_gdscript() {
        let _lock = crate::core::data_dir::test_env_lock();
        let (_tmp, root) = godot_fixture();
        let _ = handle_build(&root);
        let mut cache = crate::core::cache::SessionCache::new();
        let out = handle_symbol(
            Some("_ready"),
            &root,
            &mut cache,
            crate::tools::CrpMode::Off,
        );
        // Four `_ready` defs → a disambiguation list (or a snippet), never the
        // pre-#314 "Invalid symbol spec" / "not found" errors.
        assert!(out.contains("_ready"), "bare symbol should resolve: {out}");
        assert!(!out.contains("Invalid symbol spec"), "got: {out}");
        assert!(!out.to_lowercase().contains("not found"), "got: {out}");
        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }
}
