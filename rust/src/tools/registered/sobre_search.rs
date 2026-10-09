use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{
    McpTool, ToolContext, ToolOutput, get_bool, get_int, get_str, get_usize,
};
use crate::tool_defs::tool_def;

pub struct SobreSearchTool;

/// Which search engine a `sobre_search` call routes to. One tool, many
/// engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchAction {
    Regex,
    Semantic,
    Symbol,
    Reindex,
    FindRelated,
}

impl SearchAction {
    /// An explicit `action` wins, and must be one the schema lists; without
    /// one, the engine follows the field the caller set (`handle`/`name`,
    /// `pattern`, `file_path`+`line`, `query`), regex by default.
    fn resolve(args: &Map<String, Value>) -> Result<Self, String> {
        if let Some(a) = get_str(args, "action") {
            return match a.as_str() {
                "regex" => Ok(Self::Regex),
                "semantic" => Ok(Self::Semantic),
                "symbol" => Ok(Self::Symbol),
                "reindex" => Ok(Self::Reindex),
                "find_related" => Ok(Self::FindRelated),
                other => Err(format!(
                    "Unknown action '{other}': regex, semantic, symbol, reindex or find_related"
                )),
            };
        }
        Ok(if args.contains_key("handle") {
            Self::Symbol
        } else if args.contains_key("pattern") {
            Self::Regex
        } else if args.contains_key("name") {
            Self::Symbol
        } else if args.contains_key("file_path") && args.contains_key("line") {
            Self::FindRelated
        } else if args.contains_key("query") {
            Self::Semantic
        } else {
            Self::Regex
        })
    }
}

impl McpTool for SobreSearchTool {
    fn name(&self) -> &'static str {
        "sobre_search"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action`.
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["regex", "semantic", "symbol", "reindex", "find_related"]
                },
                "pattern": { "type": "string" },
                "query": { "type": "string" },
                "name": { "type": "string" },
                "handle": { "type": "string" },
                "path": { "type": "string" },
                "paths": { "type": "array", "items": { "type": "string" } },
                "include": { "type": "string", "description": "Glob of files to search: *.rs at any depth, src/*.rs from the root, *.{rs,ts} for several" },
                "exclude": { "type": "string", "description": "Glob of files to skip, read as include" },
                "exclude_pattern": { "type": "string" },
                "anchored": { "type": "boolean" },
                "max_results": { "type": "integer" },
                "top_k": { "type": "integer" },
                "workspace": { "type": "boolean", "description": "semantic: also search the configured search_roots" },
                "file": { "type": "string" },
                "kind": { "type": "string" },
                "file_path": { "type": "string" },
                "line": { "type": "integer" },
                "queries": {
                    "type": "array",
                    "items": { "type": "object" }
                }
            },
            "oneOf": [
                {
                    "properties": { "action": { "enum": ["regex"] } },
                    "anyOf": [{ "required": ["pattern"] }, { "required": ["queries"] }]
                },
                {
                    "properties": { "action": { "const": "semantic" } },
                    "required": ["action", "query"]
                },
                {
                    "properties": { "action": { "const": "symbol" } },
                    "required": ["action"],
                    "anyOf": [{ "required": ["name"] }, { "required": ["handle"] }]
                },
                {
                    "properties": { "action": { "const": "reindex" } },
                    "required": ["action"]
                },
                {
                    "properties": { "action": { "const": "find_related" } },
                    "required": ["action", "file_path", "line"]
                }
            ]
        });
        // Dense and hybrid ranking exist only in builds with local embeddings;
        // elsewhere the schema does not offer them.
        if cfg!(feature = "dense") {
            schema["properties"]["mode"] = json!({
                "type": "string",
                "enum": ["bm25", "dense", "hybrid"],
                "description": "semantic ranking (default bm25)"
            });
        }
        tool_def(
            "sobre_search",
            "Search code: regex(pattern, default) | semantic(query) | symbol(name|handle) | \
             reindex | find_related(file_path,line). anchored=true enables sobre_patch refs; \
             queries batches regex searches. Run sobre_compose FIRST.",
            schema,
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let action = SearchAction::resolve(args).map_err(|e| ErrorData::invalid_params(e, None))?;
        match action {
            SearchAction::Regex => handle_regex(args, ctx),
            SearchAction::Semantic => handle_semantic(args, ctx),
            SearchAction::Symbol => handle_symbol(args, ctx),
            SearchAction::Reindex => handle_reindex(ctx),
            SearchAction::FindRelated => handle_find_related(args, ctx),
        }
    }
}

/// `action=regex` (default) — exact-pattern search over one or more roots.
fn handle_regex(args: &Map<String, Value>, ctx: &ToolContext) -> Result<ToolOutput, ErrorData> {
    // Batch mode — `queries: [{pattern, include?, exclude?}]` runs multiple
    // searches in one round-trip with grouped output.
    if let Some(Value::Array(queries)) = args.get("queries") {
        return handle_batch_queries(queries, args, ctx);
    }
    let pattern = get_str(args, "pattern").ok_or_else(|| {
        ErrorData::invalid_params(
            "pattern is required. Example: sobre_search(pattern=\"fn main\", path=\"/src\")",
            None,
        )
    })?;
    let resolved = crate::server::multi_path::resolve_tool_paths(args, ctx)
        .map_err(|e| ErrorData::invalid_params(format!("ERROR: {e}"), None))?;
    let include = get_str(args, "include");
    let max = (get_int(args, "max_results").unwrap_or(20) as usize).min(500);
    // Opt-in N:hh line anchors on each hit for direct sobre_patch edits.
    let anchored = get_bool(args, "anchored").unwrap_or(false);
    // Negative filters — `exclude` (path glob, complement of `include`)
    // and `exclude_pattern` (regex dropping matching result lines, grep -v).
    let exclude = get_str(args, "exclude");
    let exclude_pattern = get_str(args, "exclude_pattern");

    let crp = ctx.crp_mode;
    let respect = true;

    if !resolved.is_multi {
        return search_single(
            &pattern,
            &resolved.roots[0],
            include.as_deref(),
            max,
            crp,
            respect,
            anchored,
            exclude.as_deref(),
            exclude_pattern.as_deref(),
        );
    }

    let per_root_max = (max / resolved.roots.len()).max(5);
    let mut combined = String::new();
    let mut total_observed: usize = 0;
    let mut total_sent: usize = 0;

    for root in &resolved.roots {
        let search_result = tokio::task::block_in_place(|| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::tools::sobre_search::handle_filtered(
                    &pattern,
                    root,
                    include.as_deref(),
                    per_root_max,
                    crp,
                    respect,
                    anchored,
                    exclude.as_deref(),
                    exclude_pattern.as_deref(),
                )
            }))
            .ok()
        });

        let Some(outcome) = search_result else {
            combined.push_str(&format!("── {root} ──\nERROR: search panicked\n\n"));
            continue;
        };
        let result = outcome.text;

        if result.trim().is_empty() {
            continue;
        }

        combined.push_str(&format!("── {root} ──\n{result}\n\n"));

        if result.starts_with("ERROR:") {
            continue;
        }

        total_observed += outcome.observed_tokens;
        total_sent += crate::core::tokens::count_tokens(&result);
    }

    if combined.is_empty() {
        combined = "No matches found across any root.".to_string();
    }

    // Savings use *observed* tokens: the modeled 2.5x native-grep baseline
    // never inflates user-facing numbers.
    let final_out = combined.clone();
    let saved = total_observed.saturating_sub(total_sent);

    Ok(ToolOutput {
        text: final_out,
        saved_tokens: saved,
        shell_outcome: None,
        content_blocks: None,
    })
}

/// Resolve the `path` arg, falling back to the project root —
/// the same precedence the former standalone semantic-search tool used.
fn resolve_path_or_root(ctx: &ToolContext) -> Result<String, ErrorData> {
    if let Some(p) = ctx.resolved_path("path") {
        Ok(p.to_string())
    } else if let Some(err) = ctx.path_error("path") {
        Err(ErrorData::invalid_params(format!("path: {err}"), None))
    } else {
        Ok(ctx.project_root.clone())
    }
}

/// Prime the per-call BM25 cache so semantic engines reuse the warmed index
/// instead of reloading it from disk (perf parity with the former tool).
fn prime_bm25_cache(ctx: &ToolContext) {
    if let Some(ref cache) = ctx.bm25_cache {
        crate::tools::impl_semantic_search::set_thread_cache(cache.clone());
    }
}

/// `action=semantic` — meaning-based search, routed to the shared core fn.
fn handle_semantic(args: &Map<String, Value>, ctx: &ToolContext) -> Result<ToolOutput, ErrorData> {
    let query = get_str(args, "query")
        .ok_or_else(|| ErrorData::invalid_params("query is required for action=semantic", None))?;
    let path = resolve_path_or_root(ctx)?;
    let top_k = get_usize(args, "top_k").unwrap_or(10).min(1000);
    // Dense and hybrid ranking exist only in builds with local embeddings,
    // where the schema declares `mode`.
    let mode = if cfg!(feature = "dense") {
        get_str(args, "mode")
    } else {
        None
    };
    let workspace = get_bool(args, "workspace").unwrap_or(false);
    prime_bm25_cache(ctx);

    let result = tokio::task::block_in_place(|| {
        crate::tools::impl_semantic_search::handle(
            &query,
            &path,
            top_k,
            ctx.crp_mode,
            mode.as_deref(),
            Some(workspace),
        )
    });

    Ok(semantic_output(&result))
}

/// When `path` or `file` is an absolute path under a different project,
/// resolve that project's root for the graph lookup. Falls back to the session
/// project_root when no cross-project path is given.
fn resolve_symbol_root(args: &Map<String, Value>, session_root: &str) -> String {
    let candidate = get_str(args, "path")
        .or_else(|| get_str(args, "file"))
        .filter(|p| std::path::Path::new(p.as_str()).is_absolute());

    // A file inside the session's project keeps that project: the detected
    // root (the nearest git root) may be an ancestor with no graph of its own.
    // Compared canonically, so case, `\` and symlinked prefixes still match.
    let canon = |p: &str| crate::core::pathutil::safe_canonicalize_or_self(std::path::Path::new(p));
    let session = canon(session_root);
    if let Some(abs_path) = candidate
        && !canon(&abs_path).starts_with(&session)
        && let Some(detected) = crate::core::protocol::detect_project_root(&abs_path)
        && canon(&detected) != session
    {
        return detected;
    }
    session_root.to_string()
}

/// `action=symbol` — one symbol's body. A `handle` (`path#name@Lline`) resolves
/// deterministically (exact, no fuzzy/disambiguation); otherwise `name` runs the
/// fuzzy lookup. Both route to the shared symbol lookup.
fn handle_symbol(args: &Map<String, Value>, ctx: &ToolContext) -> Result<ToolOutput, ErrorData> {
    // Resolve graph root from `path` when given, instead of always
    // using the sticky session project_root. This enables cross-repo symbol
    // lookup in multi-project MCP sessions.
    let effective_root = resolve_symbol_root(args, &ctx.project_root);

    if let Some(handle) = get_str(args, "handle") {
        let (result, original) =
            crate::tools::impl_symbol::render_by_handle(&handle, &effective_root);
        // Secrets are masked as `sobre_read` masks them.
        let result = crate::core::redaction::redact_text(&result);
        let sent = crate::core::tokens::count_tokens(&result);
        return Ok(ToolOutput {
            text: result,
            saved_tokens: original.saturating_sub(sent),
            shell_outcome: None,
            content_blocks: None,
        });
    }

    let name = get_str(args, "name").ok_or_else(|| {
        ErrorData::invalid_params("name or handle is required for action=symbol", None)
    })?;
    let file = get_str(args, "file");
    let kind = get_str(args, "kind");

    let (result, original) =
        crate::tools::impl_symbol::handle(&name, file.as_deref(), kind.as_deref(), &effective_root);
    let result = crate::core::redaction::redact_text(&result);
    let sent = crate::core::tokens::count_tokens(&result);
    Ok(ToolOutput {
        text: result,
        saved_tokens: original.saturating_sub(sent),
        shell_outcome: None,
        content_blocks: None,
    })
}

/// `action=reindex` — rebuild the BM25 index, routed to core.
fn handle_reindex(ctx: &ToolContext) -> Result<ToolOutput, ErrorData> {
    let path = resolve_path_or_root(ctx)?;
    prime_bm25_cache(ctx);

    let result =
        tokio::task::block_in_place(|| crate::tools::impl_semantic_search::handle_reindex(&path));

    Ok(semantic_output(&result))
}

/// `action=find_related` — context neighbors for a source location, via core.
fn handle_find_related(
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<ToolOutput, ErrorData> {
    let path = resolve_path_or_root(ctx)?;
    let top_k = get_usize(args, "top_k").unwrap_or(10).min(1000);
    let fp = get_str(args, "file_path").unwrap_or_default();
    let line = get_int(args, "line").unwrap_or(1) as usize;
    if fp.is_empty() {
        return Err(ErrorData::invalid_params(
            "find_related requires file_path and line",
            None,
        ));
    }
    prime_bm25_cache(ctx);

    let result = tokio::task::block_in_place(|| {
        crate::tools::impl_semantic_search::handle_find_related(
            &fp,
            line,
            &path,
            top_k,
            ctx.crp_mode,
        )
    });

    Ok(semantic_output(&result))
}

/// Shared `ToolOutput` shape for the semantic-engine branches (token accounting
/// is handled inside the core fns, mirroring the former standalone tool). The
/// chunk snippets they quote have their secrets masked as `sobre_read` does.
fn semantic_output(text: &str) -> ToolOutput {
    ToolOutput::simple(crate::core::redaction::redact_text(text))
}

#[allow(clippy::too_many_arguments)]
fn search_single(
    pattern: &str,
    path: &str,
    include: Option<&str>,
    max: usize,
    crp: crate::tools::CrpMode,
    respect_gitignore: bool,
    anchored: bool,
    exclude: Option<&str>,
    exclude_pattern: Option<&str>,
) -> Result<ToolOutput, ErrorData> {
    let search_result = tokio::task::block_in_place(|| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::tools::sobre_search::handle_filtered(
                pattern,
                path,
                include,
                max,
                crp,
                respect_gitignore,
                anchored,
                exclude,
                exclude_pattern,
            )
        }));
        match result {
            Ok(r) => Ok(r),
            Err(_) => Err("search task panicked"),
        }
    });

    let outcome = match search_result {
        Ok(r) => r,
        Err(e) => {
            return Err(ErrorData::internal_error(
                format!("search task failed: {e}"),
                None,
            ));
        }
    };
    let result = outcome.text;
    // Observed tokens only — the modeled native-grep baseline stays out of
    // the savings; see the multi-root branch above.
    let observed = outcome.observed_tokens;

    if result.starts_with("ERROR:") {
        return Err(ErrorData::invalid_params(result, None));
    }

    let sent = crate::core::tokens::count_tokens(&result);
    let saved = observed.saturating_sub(sent);
    let final_out = result.clone();

    Ok(ToolOutput {
        text: final_out,
        saved_tokens: saved,
        shell_outcome: None,
        content_blocks: None,
    })
}

/// Batch multi-query — runs each query independently and groups output.
fn handle_batch_queries(
    queries: &[Value],
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<ToolOutput, ErrorData> {
    if queries.is_empty() {
        return Err(ErrorData::invalid_params(
            "queries array must not be empty",
            None,
        ));
    }
    if queries.len() > 10 {
        return Err(ErrorData::invalid_params(
            "queries array limited to 10 entries",
            None,
        ));
    }

    let resolved = crate::server::multi_path::resolve_tool_paths(args, ctx)
        .map_err(|e| ErrorData::invalid_params(format!("ERROR: {e}"), None))?;
    let anchored = get_bool(args, "anchored").unwrap_or(false);
    let crp = ctx.crp_mode;
    let respect = true;
    let roots = &resolved.roots;
    // A query's own filters win; the top-level ones apply to every query
    // that sets none.
    let top_include = get_str(args, "include");
    let top_exclude = get_str(args, "exclude");
    let top_exclude_pattern = get_str(args, "exclude_pattern");
    let global_max = (get_int(args, "max_results").unwrap_or(20) as usize).min(500);
    let per_query_max = (global_max / queries.len()).max(5);
    let per_search_max = (per_query_max / roots.len()).max(5);

    let mut combined = String::new();
    let mut total_observed: usize = 0;
    let mut total_sent: usize = 0;

    for (idx, q) in queries.iter().enumerate() {
        let Some(obj) = q.as_object() else {
            combined.push_str(&format!(
                "── query {} ──\nERROR: expected object\n\n",
                idx + 1
            ));
            continue;
        };
        let Some(pattern) = get_str(obj, "pattern") else {
            combined.push_str(&format!(
                "── query {} ──\nERROR: pattern required\n\n",
                idx + 1
            ));
            continue;
        };
        let include = get_str(obj, "include").or_else(|| top_include.clone());
        let exclude = get_str(obj, "exclude").or_else(|| top_exclude.clone());
        let exclude_pattern =
            get_str(obj, "exclude_pattern").or_else(|| top_exclude_pattern.clone());

        let label = if queries.len() > 1 {
            format!(
                "── query {}: '{}' ──\n",
                idx + 1,
                truncate_query(&pattern, 40)
            )
        } else {
            String::new()
        };

        // Every root of a multi-root session is searched, as for one pattern.
        for root in roots {
            let search_result = tokio::task::block_in_place(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::tools::sobre_search::handle_filtered(
                        &pattern,
                        root,
                        include.as_deref(),
                        per_search_max,
                        crp,
                        respect,
                        anchored,
                        exclude.as_deref(),
                        exclude_pattern.as_deref(),
                    )
                }))
                .ok()
            });
            let root_label = if roots.len() > 1 {
                format!("{label}── {root} ──\n")
            } else {
                label.clone()
            };
            let Some(outcome) = search_result else {
                combined.push_str(&format!("{root_label}ERROR: search panicked\n\n"));
                continue;
            };
            if !outcome.text.trim().is_empty() {
                combined.push_str(&format!("{root_label}{}\n\n", outcome.text));
                total_observed += outcome.observed_tokens;
                total_sent += crate::core::tokens::count_tokens(&outcome.text);
            }
        }
    }

    if combined.is_empty() {
        combined = "No matches found for any query.".to_string();
    }

    let final_out = combined.clone();
    let saved = total_observed.saturating_sub(total_sent);

    Ok(ToolOutput {
        text: final_out,
        saved_tokens: saved,
        shell_outcome: None,
        content_blocks: None,
    })
}

/// Truncate a query string for display (used in batch labels).
fn truncate_query(q: &str, max: usize) -> String {
    if q.len() <= max {
        q.to_string()
    } else {
        format!("{}...", &q[..q.floor_char_boundary(max)])
    }
}

#[cfg(test)]
mod tests {
    use super::SearchAction;
    use serde_json::{Map, Value, json};

    fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .cloned()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn explicit_action_selects_engine() {
        for (action, engine) in [
            ("regex", SearchAction::Regex),
            ("semantic", SearchAction::Semantic),
            ("symbol", SearchAction::Symbol),
            ("reindex", SearchAction::Reindex),
            ("find_related", SearchAction::FindRelated),
        ] {
            assert_eq!(
                SearchAction::resolve(&args(&[("action", json!(action))])),
                Ok(engine)
            );
        }
    }

    /// Decision 7: an action the schema does not list is refused, synonyms
    /// included.
    #[test]
    fn an_unlisted_action_is_refused() {
        for action in ["grep", "pattern", "search", "related", "bogus"] {
            assert!(
                SearchAction::resolve(&args(&[("action", json!(action))])).is_err(),
                "{action}"
            );
        }
    }

    #[test]
    fn action_inferred_from_fields() {
        // A call that sets one of these fields and no action.
        assert_eq!(
            SearchAction::resolve(&args(&[("pattern", json!("fn .*"))])),
            Ok(SearchAction::Regex)
        );
        assert_eq!(
            SearchAction::resolve(&args(&[("query", json!("user auth"))])),
            Ok(SearchAction::Semantic)
        );
        assert_eq!(
            SearchAction::resolve(&args(&[("name", json!("handle"))])),
            Ok(SearchAction::Symbol)
        );
        assert_eq!(
            SearchAction::resolve(&args(&[("file_path", json!("a.rs")), ("line", json!(10))])),
            Ok(SearchAction::FindRelated)
        );
    }

    #[test]
    fn handle_infers_symbol_action() {
        // A bare `handle` (no action) must route to the symbol engine.
        assert_eq!(
            SearchAction::resolve(&args(&[("handle", json!("src/lib.rs#Config::load@L22"))])),
            Ok(SearchAction::Symbol)
        );
    }

    #[test]
    fn pattern_wins_over_query() {
        // A regex caller that also carries a stray query must stay regex.
        assert_eq!(
            SearchAction::resolve(&args(&[("pattern", json!("x")), ("query", json!("y"))])),
            Ok(SearchAction::Regex)
        );
        // Nothing recognizable → default regex (the empty-call default).
        assert_eq!(SearchAction::resolve(&args(&[])), Ok(SearchAction::Regex));
    }

    /// The top-level `include` applies to every query that sets none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_level_include_applies_to_each_query() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "needle\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "needle\n").unwrap();
        let ctx = super::ToolContext {
            project_root: crate::core::pathutil::safe_canonicalize_or_self(dir.path())
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        let a = args(&[
            ("queries", json!([{ "pattern": "needle" }])),
            ("include", json!("*.rs")),
        ]);
        let queries = a["queries"].as_array().unwrap().clone();
        let out = super::handle_batch_queries(&queries, &a, &ctx)
            .unwrap()
            .text;
        assert!(out.contains("a.rs"), "{out}");
        assert!(!out.contains("b.py"), "{out}");
    }

    /// A file inside the session's project keeps that project even when
    /// the session root is spelled through a symlink.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_session_root_keeps_its_files() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let project = repo.join("sub");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("a.rs"), "fn a() {}\n").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&project, &link).unwrap();

        let file = project.canonicalize().unwrap().join("a.rs");
        let session = link.to_string_lossy().to_string();
        let a = args(&[("path", json!(file.to_string_lossy()))]);
        assert_eq!(super::resolve_symbol_root(&a, &session), session);
    }
}
