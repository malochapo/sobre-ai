use serde_json::{Map, Value};

use crate::server::tool_trait::{ToolContext, get_str_array};

#[derive(Debug)]
pub struct ResolvedPaths {
    pub roots: Vec<String>,
    pub is_multi: bool,
}

/// Resolve tool paths with multi-root support.
///
/// Priority:
/// 0. `repo` argument (multi-repo alias → specific root)
/// 1. `paths` array argument (explicit multi-root)
/// 2. `path` string argument (single root, pre-resolved by dispatch)
/// 3. Session `extra_roots` (default multi-root from config/MCP)
/// 4. Fallback to `"."` (project root)
///
/// Returns `Err` when an **explicit** `path`/`paths` argument was supplied but
/// could not be resolved. Silently falling back to the project root in that case made
/// `ctx_tree path=/outside/repo` return the whole project tree (#401).
pub fn resolve_tool_paths(
    args: &Map<String, Value>,
    ctx: &ToolContext,
) -> Result<ResolvedPaths, String> {
    if let Some(paths) = get_str_array(args, "paths")
        && !paths.is_empty()
    {
        let resolved = resolve_paths_sync(ctx, &paths);
        if !resolved.is_empty() {
            return Ok(ResolvedPaths {
                is_multi: resolved.len() > 1,
                roots: resolved,
            });
        }
        // The caller explicitly listed paths but none resolved — surface
        // the failure instead of scanning the project root (#401).
        return Err(format!(
            "none of the requested paths could be resolved: {}",
            paths.join(", ")
        ));
    }

    // #846: `file_path` is an alias for `path` in ctx_search — agents
    // (especially Claude Code) send it expecting file-level scoping.
    // Without this fallback the parameter is silently ignored and the
    // search runs on the project root.
    for key in &["path", "file_path"] {
        if let Some(p) = ctx.resolved_path(key) {
            return Ok(ResolvedPaths {
                roots: vec![p.to_string()],
                is_multi: false,
            });
        }
        // An explicit path the dispatcher could not resolve lands in
        // `path_errors`. Do NOT fall back to the project root — return the
        // resolution error rather than silently an unrelated tree (#401).
        if let Some(detail) = ctx.path_error(key) {
            return Err(detail.to_string());
        }
    }

    if let Some(session_lock) = ctx.session.as_ref() {
        let extra = tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async { session_lock.read().await.extra_roots.clone() })
        });
        if !extra.is_empty() {
            let mut roots = vec![ctx.project_root.clone()];
            roots.extend(
                extra
                    .iter()
                    .filter(|r| std::path::Path::new(r).is_dir())
                    .cloned(),
            );
            if roots.len() > 1 {
                return Ok(ResolvedPaths {
                    is_multi: true,
                    roots,
                });
            }
        }
    }

    Ok(ResolvedPaths {
        roots: vec![".".to_string()],
        is_multi: false,
    })
}

fn resolve_paths_sync(ctx: &ToolContext, raw: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(raw.len());
    for p in raw {
        match ctx.resolve_path_sync(p) {
            Ok(resolved) => out.push(resolved),
            Err(e) => {
                tracing::warn!("multi-path resolve failed for {p}: {e}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_ctx() -> ToolContext {
        ToolContext {
            project_root: "/test/project".to_string(),
            extra_roots: Vec::new(),
            resolved_paths: std::collections::HashMap::new(),
            crp_mode: crate::tools::CrpMode::Off,
            cache: None,
            session: None,
            tool_calls: None,
            path_errors: std::collections::HashMap::new(),
            bm25_cache: None,
            progress_sender: None,
        }
    }

    #[test]
    fn fallback_to_dot_when_nothing_set() {
        let args = Map::new();
        let ctx = test_ctx();
        let result = resolve_tool_paths(&args, &ctx).expect("no explicit path → default");
        assert_eq!(result.roots, vec!["."]);
        assert!(!result.is_multi);
    }

    #[test]
    fn uses_resolved_path_when_present() {
        let args = Map::new();
        let mut ctx = test_ctx();
        ctx.resolved_paths
            .insert("path".to_string(), "/resolved/dir".to_string());
        let result = resolve_tool_paths(&args, &ctx).expect("resolved path");
        assert_eq!(result.roots, vec!["/resolved/dir"]);
        assert!(!result.is_multi);
    }

    #[test]
    fn empty_paths_array_falls_back() {
        let mut args = Map::new();
        args.insert("paths".to_string(), json!([]));
        let mut ctx = test_ctx();
        ctx.resolved_paths
            .insert("path".to_string(), "/fallback".to_string());
        let result = resolve_tool_paths(&args, &ctx).expect("empty paths → fallback");
        assert_eq!(result.roots, vec!["/fallback"]);
        assert!(!result.is_multi);
    }

    // #401: an explicit `path` the dispatcher could not resolve must surface
    // the error — NOT silently
    // fall back to the project root and return an unrelated tree.
    #[test]
    fn explicit_unresolvable_path_errors_instead_of_root_fallback() {
        let mut args = Map::new();
        args.insert(
            "path".to_string(),
            json!("/home/jules/.claude/skills/mpm-config"),
        );
        let mut ctx = test_ctx();
        // Dispatcher could not resolve it → recorded in path_errors, absent
        // from resolved_paths.
        ctx.path_errors
            .insert("path".to_string(), "path contains a null byte".to_string());
        let err = resolve_tool_paths(&args, &ctx)
            .expect_err("an unresolvable explicit path must be an error");
        assert!(
            err.contains("null byte"),
            "error must carry the resolution failure: {err}"
        );
    }

    // An explicit `paths` entry outside the project root resolves: sobre
    // does not decide what the agent may read (INVARIANTS.md §6).
    #[test]
    fn explicit_paths_outside_root_resolve() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        let outside = base.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let mut ctx = test_ctx();
        ctx.project_root = root.to_string_lossy().into_owned();

        let mut args = Map::new();
        args.insert(
            "paths".to_string(),
            json!([outside.to_string_lossy().into_owned()]),
        );
        let resolved = resolve_tool_paths(&args, &ctx).expect("outside path resolves");
        assert_eq!(resolved.roots.len(), 1);
        assert!(!resolved.is_multi);
    }
}
