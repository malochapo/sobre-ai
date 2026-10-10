use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::cli::args::CallArgs;
use crate::core::cache::SessionCache;
use crate::core::error::DispatchError;
use crate::core::session::SessionState;
use crate::server::registry::build_registry;
use crate::server::tool_trait::ToolContext;

/// CLI-level failure — distinct from a tool's *functional* result (a tool that
/// failed still prints its text to stdout, and exits 1). These variants are
/// wrong *usage* of `call`.
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
            CallError::Usage(m) => write!(f, "error: {m}"),
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

/// What `sobre call` runs: the tool, the project and the JSON arguments.
struct Resolved {
    tool: String,
    project_root: String,
    json: String,
}

fn resolve(args: &CallArgs) -> Result<Resolved, CallError> {
    // Without the flag, the project around the current directory, as for the
    // other commands; a relative root (`.`) is made absolute first, so the
    // broad-root check sees the real directory.
    let project_root = match &args.project_root {
        Some(p) => {
            let path = Path::new(p);
            let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            crate::core::pathutil::safe_canonicalize_or_self(&abs)
                .to_string_lossy()
                .into_owned()
        }
        None => crate::cli::common::project_root(None),
    };

    let json = match (&args.json, &args.json_file) {
        (Some(_), Some(_)) => {
            return Err(CallError::Usage(
                "use either --json or --json-file, not both".into(),
            ));
        }
        (Some(j), None) => j.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|e| CallError::Usage(format!("cannot read --json-file '{path}': {e}")))?,
        (None, None) => "{}".to_string(),
    };

    Ok(Resolved {
        tool: args.tool.clone(),
        project_root,
        json,
    })
}

fn oneshot_ctx(
    project_root: String,
    resolved_paths: HashMap<String, String>,
    path_errors: HashMap<String, String>,
) -> ToolContext {
    // An empty session and a fresh file cache, as a starting MCP server has
    //: the tools that track reads or memory (`sobre_read`,
    // `sobre_shell`, `sobre_session`, `sobre_knowledge`) refuse to run without
    // them. The session is stamped with the canonical root so a later load
    // matches it.
    let mut session = SessionState::new();
    session.project_root = Some(
        crate::core::pathutil::safe_canonicalize_or_self(Path::new(&project_root))
            .to_string_lossy()
            .into_owned(),
    );
    ToolContext {
        project_root,
        resolved_paths,
        path_errors,
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

/// What the tool returned: its text, and whether the server would mark it
/// `isError`.
#[derive(Debug)]
pub(crate) struct Called {
    pub text: String,
    pub is_error: bool,
}

/// Core, testable entry point. Returns what the tool returned; `CallError`
/// only for CLI-usage problems (never for functional tool errors).
pub(crate) fn run_call(args: &CallArgs) -> Result<Called, CallError> {
    let parsed = resolve(args)?;

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

    // Resolve the path-like arguments as the server's dispatcher does, so
    // handlers that read `ctx.resolved_path(…)` see the same values; a failure
    // goes to the handler as a path error, as it does over MCP.
    let mut resolved_paths = HashMap::new();
    let mut path_errors = HashMap::new();
    for key in crate::server::dispatch::PATH_LIKE_KEYS {
        let Some(p) = args_map.get(*key).and_then(Value::as_str) else {
            continue;
        };
        match crate::core::path_resolve::resolve_tool_path(Some(&parsed.project_root), None, p) {
            // `resolve_tool_path` passes "." / "" through unchanged, which a
            // handler would then resolve against its CWD — not project_root.
            Ok(abs) if abs.is_empty() || abs == "." => {
                resolved_paths.insert((*key).to_string(), parsed.project_root.clone());
            }
            Ok(abs) => {
                resolved_paths.insert((*key).to_string(), abs);
            }
            Err(e) => {
                path_errors.insert((*key).to_string(), e);
            }
        }
    }

    let ctx = oneshot_ctx(parsed.project_root.clone(), resolved_paths, path_errors);

    let registry = build_registry();
    let tool = registry
        .get_arc(&parsed.tool)
        .ok_or_else(|| CallError::UnknownTool(parsed.tool.clone()))?;

    // The process exits after this call, so what `sobre_session` recorded (a
    // task, a finding, a decision) is saved now, as the server would save it.
    // Other tools' session bookkeeping (files read, an inferred task) says what
    // an agent has seen; a shell call is not the agent, so it is dropped.
    let save_session = parsed.tool == "sobre_session";
    let tool_name = parsed.tool.clone();
    let reports_failures = tool.text_reports_failures();

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

    // Recorded in the usage ledger as every other path records it.
    let sent = crate::core::tokens::count_tokens(&output.text);
    crate::core::usage::record_compression(
        crate::core::usage::Surface::Call,
        &tool_name,
        sent + output.saved_tokens,
        sent,
    );

    // As the server's dispatch decides `isError`: from the shell outcome, or
    // from the text of a tool whose text says when it failed.
    let is_error = match output.shell_outcome {
        Some(outcome) => outcome.is_error(),
        None => reports_failures && crate::server::tool_trait::reports_failure(&output.text),
    };
    Ok(Called {
        text: output.text,
        is_error,
    })
}

/// Thin CLI wrapper: the tool's text on stdout, exit 1 when the server would
/// mark it `isError`; a usage error on stderr, exit 2.
pub(crate) fn cmd_call(args: &CallArgs) {
    match run_call(args) {
        Ok(called) => {
            println!("{}", called.text);
            if called.is_error {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call_args(tool: &str, project_root: Option<&str>, json: Option<&str>) -> CallArgs {
        CallArgs {
            tool: tool.to_string(),
            project_root: project_root.map(str::to_string),
            json: json.map(str::to_string),
            json_file: None,
        }
    }

    #[test]
    fn unknown_tool_is_cli_error() {
        let tmp = std::env::temp_dir().to_string_lossy().to_string();
        let args = call_args("definitely_not_a_tool", Some(&tmp), Some("{}"));
        let err = run_call(&args).expect_err("expected unknown-tool error");
        assert!(matches!(err, CallError::UnknownTool(_)), "got {err:?}");
    }

    #[test]
    fn invalid_json_is_cli_error() {
        let tmp = std::env::temp_dir().to_string_lossy().to_string();
        let args = call_args("sobre_tree", Some(&tmp), Some("{not json"));
        let err = run_call(&args).expect_err("expected bad-json error");
        assert!(matches!(err, CallError::BadJson(_)), "got {err:?}");
    }

    #[test]
    fn unsafe_root_is_rejected() {
        let args = call_args("sobre_tree", Some("/"), Some("{}"));
        let err = run_call(&args).expect_err("expected unsafe-root error");
        assert!(matches!(err, CallError::UnsafeRoot(_)), "got {err:?}");
    }

    /// No `--project-root` means the project around the current
    /// directory, and `.` is the current directory made absolute.
    #[test]
    fn project_root_defaults_to_cwd_and_dot_is_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let bare = resolve(&call_args("sobre_tree", None, None)).unwrap();
        assert!(cwd.starts_with(&bare.project_root), "{}", bare.project_root);
        let dot = resolve(&call_args("sobre_tree", Some("."), None)).unwrap();
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

        let root = dir.to_string_lossy().to_string();
        let args = call_args("sobre_tree", Some(&root), Some(r#"{"path": "."}"#));
        let out = run_call(&args).expect("dispatch should succeed").text;
        assert!(
            out.contains("MARKER_FILE.txt"),
            "tree output missing marker:\n{out}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A tool that failed is a call that worked: its text, marked as the
    /// server marks it, so `cmd_call` exits 1.
    #[test]
    fn a_failed_tool_is_marked_as_an_error() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_string_lossy().to_string();
        let call = |tool: &str, json: &str| {
            run_call(&call_args(tool, Some(&root), Some(json))).expect("the call itself works")
        };
        let failed = call("sobre_shell", r#"{"command": "exit 3"}"#);
        assert!(failed.is_error, "{}", failed.text);
        let fine = call("sobre_shell", r#"{"command": "echo ok"}"#);
        assert!(!fine.is_error, "{}", fine.text);
    }

    /// A tool whose text says it failed (an `ERROR` or a `CONFLICT`) is marked
    /// as the server marks it, so `cmd_call` exits 1.
    #[test]
    fn a_failure_the_text_reports_is_marked_as_an_error() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_string_lossy().to_string();
        let file = project.path().join("a.txt");
        std::fs::write(&file, "one\n").unwrap();
        let path = file.to_string_lossy();
        // serde_json escapes the path's backslashes on Windows.
        let patch = |old: &str, new: &str| {
            serde_json::json!({"path": path, "op": "replace_unique", "old_text": old, "new_text": new})
                .to_string()
        };
        let failed = run_call(&call_args(
            "sobre_patch",
            Some(&root),
            Some(&patch("absent", "x")),
        ))
        .expect("the call itself works");
        assert!(failed.is_error, "{}", failed.text);
        let fine = run_call(&call_args(
            "sobre_patch",
            Some(&root),
            Some(&patch("one", "two")),
        ))
        .expect("the call itself works");
        assert!(!fine.is_error, "{}", fine.text);
    }

    /// Each call starts an empty session; what one call saved, `load`
    /// restores.
    #[test]
    fn a_saved_session_is_loaded_on_request() {
        let _data = crate::core::data_dir::isolated_data_dir();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_string_lossy().to_string();
        let call = |json: &str| {
            run_call(&call_args("sobre_session", Some(&root), Some(json)))
                .expect("sobre_session call")
                .text
        };

        call(r#"{"action": "task", "value": "wire the parser"}"#);
        let status = call(r#"{"action": "status"}"#);
        assert!(!status.contains("wire the parser"), "{status}");
        let loaded = call(r#"{"action": "load"}"#);
        assert!(
            loaded.contains("wire the parser"),
            "the task saved by the first call is lost:\n{loaded}"
        );
    }
}
