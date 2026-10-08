use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::core::cache::SessionCache;
use crate::core::error::DispatchError;
use crate::core::session::SessionState;
use crate::server::registry::build_registry;
use crate::server::tool_trait::ToolContext;

/// CLI-level failure — distinct from a tool's *functional* result (a tool that
/// returns "ERROR:" text is a successful invocation and
/// goes to stdout with exit 0). These variants are wrong *usage* of `call`.
#[derive(Debug)]
pub(crate) enum CallError {
    Usage(String),
    UnknownTool(String),
    BadJson(String),
    UnsafeRoot(String),
    Dispatch(DispatchError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Usage(m) => write!(
                f,
                "error: {m}\nusage: sobre call <tool> [--project-root <path>] --json '<json>' [--json-file <path>]"
            ),
            CallError::UnknownTool(t) => write!(f, "error: unknown tool '{t}'"),
            CallError::BadJson(m) => write!(f, "error: invalid --json: {m}"),
            CallError::UnsafeRoot(p) => {
                write!(
                    f,
                    "error: refusing broad/unsafe project root '{p}' (pass --project-root)"
                )
            }
            CallError::Dispatch(e) => write!(f, "error: {e}"),
        }
    }
}

impl CallError {
    /// All CLI-usage errors map to exit code 2 (distinct from tool functional
    /// errors which exit 0). Reserved 1 for unexpected internal failures.
    /// Takes `&self` so future variants can map to differentiated exit codes
    /// without touching call sites.
    #[allow(clippy::unused_self)]
    pub(crate) fn exit_code(&self) -> i32 {
        2
    }
}

/// Parsed CLI invocation for `sobre call`.
struct CallArgs {
    tool: String,
    project_root: String,
    json: String,
}

fn parse_args(args: &[String]) -> Result<CallArgs, CallError> {
    let mut tool: Option<String> = None;
    let mut project_root: Option<String> = None;
    let mut json: Option<String> = None;
    let mut json_file: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project-root" => {
                i += 1;
                project_root = Some(
                    args.get(i)
                        .ok_or_else(|| CallError::Usage("--project-root needs a value".into()))?
                        .clone(),
                );
            }
            "--json" => {
                i += 1;
                json = Some(
                    args.get(i)
                        .ok_or_else(|| CallError::Usage("--json needs a value".into()))?
                        .clone(),
                );
            }
            "--json-file" => {
                i += 1;
                json_file = Some(
                    args.get(i)
                        .ok_or_else(|| CallError::Usage("--json-file needs a value".into()))?
                        .clone(),
                );
            }
            other if other.starts_with("--") => {
                return Err(CallError::Usage(format!("unknown flag '{other}'")));
            }
            _ => {
                if tool.is_none() {
                    tool = Some(args[i].clone());
                } else {
                    return Err(CallError::Usage(format!(
                        "unexpected argument '{}'",
                        args[i]
                    )));
                }
            }
        }
        i += 1;
    }

    let tool = tool.ok_or_else(|| CallError::Usage("missing <tool>".into()))?;
    // Without the flag, the project around the current directory, as for the
    // other commands; a relative root (`.`) is made absolute first, so the
    // broad-root check sees the real directory (C8).
    let project_root = match project_root {
        Some(p) => {
            let path = Path::new(&p);
            let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            crate::core::pathutil::safe_canonicalize_or_self(&abs)
                .to_string_lossy()
                .into_owned()
        }
        None => crate::cli::common::detect_project_root(&[]),
    };

    let json = match (json, json_file) {
        (Some(_), Some(_)) => {
            return Err(CallError::Usage(
                "use either --json or --json-file, not both".into(),
            ));
        }
        (Some(j), None) => j,
        (None, Some(path)) => std::fs::read_to_string(&path)
            .map_err(|e| CallError::Usage(format!("cannot read --json-file '{path}': {e}")))?,
        (None, None) => "{}".to_string(),
    };

    Ok(CallArgs {
        tool,
        project_root,
        json,
    })
}

fn oneshot_ctx(project_root: String, resolved_paths: HashMap<String, String>) -> ToolContext {
    // The project's latest session and a fresh file cache, as a starting MCP
    // server has: the tools that track reads or memory (`sobre_read`,
    // `sobre_shell`, `sobre_session`, `sobre_knowledge`) refuse to run without them.
    // The session is stamped with the canonical root so a later load matches it.
    let mut session = SessionState::load_latest_for_project_root(&project_root).unwrap_or_default();
    session.project_root = Some(
        crate::core::pathutil::safe_canonicalize_or_self(Path::new(&project_root))
            .to_string_lossy()
            .into_owned(),
    );
    // What is on disk is saved; only this call's changes count as unsaved.
    session.stats.unsaved_changes = 0;
    ToolContext {
        project_root,
        resolved_paths,
        cache: Some(Arc::new(RwLock::new(SessionCache::new()))),
        session: Some(Arc::new(RwLock::new(session))),
        // Give tools a real (empty) resident BM25 cache, as the MCP server does.
        // Without it, cache-aware tools (e.g. sobre_compose's specificity ranking)
        // silently take their cold-start fallback and CLI `call` stops mirroring
        // the server path it exists to reproduce.
        bm25_cache: Some(Arc::new(std::sync::Mutex::new(None))),
        ..Default::default()
    }
}

/// Core, testable entry point. Returns the tool's stdout text on success;
/// `CallError` only for CLI-usage problems (never for functional tool errors).
pub(crate) fn run_call(args: &[String]) -> Result<String, CallError> {
    let parsed = parse_args(args)?;

    // Same defense as MCP root resolution: never operate on a broad/unsafe root.
    if crate::core::pathutil::is_broad_or_unsafe_root(Path::new(&parsed.project_root)) {
        return Err(CallError::UnsafeRoot(parsed.project_root));
    }

    let value: Value =
        serde_json::from_str(&parsed.json).map_err(|e| CallError::BadJson(e.to_string()))?;
    let args_map: Map<String, Value> = match value {
        Value::Object(m) => m,
        _ => return Err(CallError::BadJson("expected a JSON object".into())),
    };

    // Pre-resolve a `path` string arg into resolved_paths so handlers that read
    // ctx.resolved_path("path") (e.g. sobre_tree, require_resolved_path) work.
    // Without this, multi_path falls back to "." (CWD), not project_root.
    let mut resolved_paths = HashMap::new();
    if let Some(p) = args_map.get("path").and_then(Value::as_str) {
        match crate::core::path_resolve::resolve_tool_path(Some(&parsed.project_root), None, p) {
            Ok(abs) => {
                // `resolve_tool_path` passes "." / "" through unchanged, which a
                // handler would then resolve against its CWD — not project_root.
                // Pin those to the explicit project_root so handlers operate on
                // the root we were given, never the process CWD.
                let resolved = if abs.is_empty() || abs == "." {
                    parsed.project_root.clone()
                } else {
                    abs
                };
                resolved_paths.insert("path".to_string(), resolved);
            }
            Err(e) => {
                return Err(CallError::Dispatch(DispatchError::PathResolution {
                    message: e,
                }));
            }
        }
    }

    let ctx = oneshot_ctx(parsed.project_root.clone(), resolved_paths);

    let registry = build_registry();
    let tool = registry
        .get_arc(&parsed.tool)
        .ok_or_else(|| CallError::UnknownTool(parsed.tool.clone()))?;

    // The process exits after this call, so what `sobre_session` recorded (a
    // task, a finding, a decision) is saved now, as the server would save it.
    // Other tools' session bookkeeping (files read, an inferred task) says what
    // an agent has seen; a shell call is not the agent, so it is dropped.
    let save_session = parsed.tool == "sobre_session";

    // Run the synchronous handler on the blocking pool of a runtime, as the
    // server does: handlers that lock the session block on it there.
    let output = super::dispatch::run_async(async move {
        tokio::task::spawn_blocking(move || {
            let output = tool.handle(&args_map, &ctx);
            if save_session && let Some(session) = &ctx.session {
                let mut session = session.blocking_write();
                if session.stats.unsaved_changes > 0 {
                    let _ = session.save();
                }
            }
            output
        })
        .await
    })
    .map_err(|e| e.to_string())
    .and_then(|r| r.map_err(|e| e.to_string()))
    .map_err(|message| CallError::Dispatch(DispatchError::Tool { message }))?;

    Ok(output.text)
}

/// Thin CLI wrapper: print result to stdout (exit 0, even for functional
/// "ERROR:" output), or usage error to stderr (exit 2).
pub(crate) fn cmd_call(args: &[String]) {
    match run_call(args) {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(e.exit_code());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_tool_is_cli_error() {
        let args = vec![
            "definitely_not_a_tool".to_string(),
            "--project-root".to_string(),
            std::env::temp_dir().to_string_lossy().to_string(),
            "--json".to_string(),
            "{}".to_string(),
        ];
        let err = run_call(&args).expect_err("expected unknown-tool error");
        assert!(matches!(err, CallError::UnknownTool(_)), "got {err:?}");
    }

    #[test]
    fn invalid_json_is_cli_error() {
        let args = vec![
            "sobre_tree".to_string(),
            "--project-root".to_string(),
            std::env::temp_dir().to_string_lossy().to_string(),
            "--json".to_string(),
            "{not json".to_string(),
        ];
        let err = run_call(&args).expect_err("expected bad-json error");
        assert!(matches!(err, CallError::BadJson(_)), "got {err:?}");
    }

    #[test]
    fn unsafe_root_is_rejected() {
        let args = vec![
            "sobre_tree".to_string(),
            "--project-root".to_string(),
            "/".to_string(),
            "--json".to_string(),
            "{}".to_string(),
        ];
        let err = run_call(&args).expect_err("expected unsafe-root error");
        assert!(matches!(err, CallError::UnsafeRoot(_)), "got {err:?}");
    }

    /// C8: no `--project-root` means the project around the current
    /// directory, and `.` is the current directory made absolute.
    #[test]
    fn project_root_defaults_to_cwd_and_dot_is_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let bare = parse_args(&["sobre_tree".to_string()]).unwrap();
        assert!(cwd.starts_with(&bare.project_root), "{}", bare.project_root);
        let dot = parse_args(&[
            "sobre_tree".to_string(),
            "--project-root".to_string(),
            ".".to_string(),
        ])
        .unwrap();
        assert!(
            Path::new(&dot.project_root).is_absolute(),
            "{}",
            dot.project_root
        );
        assert!(!crate::core::pathutil::is_broad_or_unsafe_root(Path::new(
            &dot.project_root
        )));
    }

    #[test]
    fn happy_path_dispatches_to_real_tool() {
        let dir = std::env::temp_dir().join(format!("sobre-call-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("MARKER_FILE.txt");
        std::fs::write(&marker, b"x").unwrap();

        let args = vec![
            "sobre_tree".to_string(),
            "--project-root".to_string(),
            dir.to_string_lossy().to_string(),
            "--json".to_string(),
            r#"{"path": "."}"#.to_string(),
        ];
        let out = run_call(&args).expect("dispatch should succeed");
        assert!(
            out.contains("MARKER_FILE.txt"),
            "tree output missing marker:\n{out}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_changes_persist_across_calls() {
        let _data = crate::core::data_dir::isolated_data_dir();
        let project = tempfile::tempdir().unwrap();
        let call = |json: &str| {
            run_call(&[
                "sobre_session".to_string(),
                "--project-root".to_string(),
                project.path().to_string_lossy().to_string(),
                "--json".to_string(),
                json.to_string(),
            ])
            .expect("sobre_session call")
        };

        call(r#"{"action": "task", "value": "wire the parser"}"#);
        let status = call(r#"{"action": "status"}"#);
        assert!(
            status.contains("wire the parser"),
            "the task set by the first call is lost:\n{status}"
        );
    }
}
