use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use serde_json::Value;

use crate::server::tool_trait::{McpTool, ShellOutcome, ToolContext, ToolOutput};
use crate::tools::SobreServer;
use rmcp::model::ContentBlock;

impl SobreServer {
    /// Returns (output_text, saved_tokens, shell_outcome, content_blocks).
    /// saved_tokens > 0 indicates the tool already applied internal compression
    /// (shell engine, cache deltas, etc.). shell_outcome is `Some` for
    /// shell-executing tools so the caller can populate MCP error metadata.
    /// content_blocks carries image/binary MCP blocks when present.
    pub(super) async fn dispatch_tool(
        &self,
        name: &str,
        args: Option<&serde_json::Map<String, Value>>,
        progress: Option<crate::server::progress::ProgressSender>,
    ) -> Result<
        (
            String,
            usize,
            Option<ShellOutcome>,
            Option<Vec<ContentBlock>>,
        ),
        ErrorData,
    > {
        if let Some(tool) = self.registry.get_arc(name) {
            let empty = serde_json::Map::new();
            let args_map = args.unwrap_or(&empty);
            let root_before = self.session.read().await.project_root.clone();
            let (resolved_paths, path_errors) = self.resolve_path_args(args_map).await;
            let (project_root, shell_cwd) = {
                let session = self.session.read().await;
                (
                    session.project_root.clone().unwrap_or_default(),
                    session.shell_cwd.clone(),
                )
            };
            // An absolute path can re-root the session (`resolve_path`): say so.
            let reroot_note = (root_before.as_deref() != Some(project_root.as_str())
                && !project_root.is_empty()
                && !tool.produces_machine_readable(Some(args_map)))
            .then(|| {
                format!(
                    "[sobre: project root is now {project_root}; relative paths resolve there]\n"
                )
            });

            // Lazy, demand-driven index warming: only tools that actually
            // need a prebuilt index trigger a (background, once-per-root) scan.
            // The first heavy pre-warm also warms any configured extra roots once.
            if !project_root.is_empty()
                && crate::core::index_orchestrator::ensure_warm_for_tool(&project_root, name)
            {
                let extra_roots = self.session.read().await.extra_roots.clone();
                if !extra_roots.is_empty() {
                    let primary = project_root.clone();
                    std::thread::spawn(move || {
                        crate::core::index_orchestrator::ensure_extra_roots_background(
                            &primary,
                            &extra_roots,
                        );
                    });
                }
            }

            let crp_mode = crate::tools::CrpMode::effective();
            let ctx = crate::server::tool_trait::ToolContext {
                project_root,
                shell_cwd,
                resolved_paths,
                crp_mode,
                cache: Some(self.cache.clone()),
                session: Some(self.session.clone()),
                path_errors,
                bm25_cache: Some(self.bm25_cache.clone()),
                progress_sender: progress,
            };
            // Run the (synchronous) handler on the dedicated blocking pool under
            // a watchdog deadline. `block_in_place` would pin one of the
            // few core workers and — being synchronous — cannot be interrupted
            // by `tokio::time::timeout` from the same task, so a hung handler
            // would silently swallow the JSON-RPC response and the MCP client
            // would crash with "Cannot read properties of undefined (reading
            // 'invoke')". `spawn_blocking` keeps the core workers free and lets
            // the watchdog always return a response.
            let reports_failures = tool.text_reports_failures();
            let mut output = self.run_tool_handler(name, tool, args_map, ctx).await?;
            if let Some(note) = reroot_note {
                output.text.insert_str(0, &note);
            }
            if output.shell_outcome.is_none()
                && reports_failures
                && crate::server::tool_trait::reports_failure(&output.text)
            {
                output.shell_outcome = Some(crate::server::tool_trait::ShellOutcome::Failed);
            }

            // Image/binary content blocks bypass all text processing.
            if output.content_blocks.is_some() {
                return Ok((
                    String::new(),
                    0,
                    output.shell_outcome,
                    output.content_blocks,
                ));
            }

            let saved = output.saved_tokens;
            let raw_text = output.text;
            return Ok((raw_text, saved, output.shell_outcome, None));
        }

        // Unknown tool: suggest the closest registered name so a typo
        // (`sobre_raed`) costs the agent a hint instead of a blind retry.
        let suggestion = crate::core::levenshtein::closest(name, self.registry.names())
            .map(|s| format!(" — did you mean '{s}'?"))
            .unwrap_or_default();
        Err(ErrorData::invalid_params(
            format!("Unknown tool: {name}{suggestion}"),
            None,
        ))
    }

    /// Resolve every path-like argument through [`Self::resolve_path`]:
    /// the resolved paths by key, and the errors by key.
    async fn resolve_path_args(
        &self,
        args_map: &serde_json::Map<String, Value>,
    ) -> (
        std::collections::HashMap<String, String>,
        std::collections::HashMap<String, String>,
    ) {
        let mut resolved_paths = std::collections::HashMap::new();
        let mut path_errors: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for key in PATH_LIKE_KEYS {
            if let Some(val) = args_map.get(*key) {
                if let Some(raw) = val.as_str() {
                    match self.resolve_path(raw).await {
                        Ok(resolved) => {
                            if !["path", "project_root"].contains(key) {
                                tracing::trace!(
                                    "[paths] resolved non-standard path key '{key}': {raw} -> {resolved}"
                                );
                            }
                            resolved_paths.insert(key.to_string(), resolved);
                        }
                        Err(e) => {
                            tracing::debug!(
                                "[dispatch] path resolution failed for '{key}' = '{raw}': {e}"
                            );
                            path_errors.insert(key.to_string(), e);
                        }
                    }
                } else {
                    let type_name = match val {
                        serde_json::Value::Number(_) => "number",
                        serde_json::Value::Bool(_) => "boolean",
                        serde_json::Value::Array(_) => "array",
                        serde_json::Value::Object(_) => "object",
                        serde_json::Value::Null => "null",
                        serde_json::Value::String(_) => unreachable!(),
                    };
                    path_errors.insert(
                        key.to_string(),
                        format!("{key} must be a string, got {type_name}"),
                    );
                }
            }
        }

        (resolved_paths, path_errors)
    }

    /// Execute a (synchronous) tool handler on the blocking pool under a
    /// watchdog deadline.
    ///
    /// The handler is `Send + 'static` (it owns an `Arc<dyn McpTool>`, a cloned
    /// arg map and the `ToolContext`), so it runs via `spawn_blocking` on the
    /// dedicated blocking-thread pool. That keeps the few core async workers free
    /// to keep driving the stdio JSON-RPC loop, and — crucially — lets the
    /// watchdog `timeout` actually fire (a synchronous `block_in_place` on the
    /// same task can never be timed out, because the task's own timer cannot be
    /// polled while it blocks).
    async fn run_tool_handler(
        &self,
        name: &str,
        tool: Arc<dyn McpTool>,
        args_map: &serde_json::Map<String, Value>,
        ctx: ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let args_owned = args_map.clone();
        let join = tokio::task::spawn_blocking(move || tool.handle(&args_owned, &ctx));
        Self::watchdog_join(name, join, Self::handler_watchdog(name)).await
    }

    /// Await a blocking handler's join handle, enforcing an optional watchdog.
    ///
    /// On timeout the join handle is **aborted** — the blocking-pool thread
    /// receives a cancellation signal so it does not permanently leak a pool
    /// slot. A clean error is always returned so the MCP client gets a
    /// JSON-RPC reply. A handler panic is isolated and surfaced as an error too.
    async fn watchdog_join(
        name: &str,
        join: tokio::task::JoinHandle<Result<ToolOutput, ErrorData>>,
        watchdog: Option<Duration>,
    ) -> Result<ToolOutput, ErrorData> {
        let Some(limit) = watchdog else {
            return Self::unwrap_join(name, join.await);
        };
        tokio::select! {
            joined = join => Self::unwrap_join(name, joined),
            () = tokio::time::sleep(limit) => {
                // The JoinHandle was moved into `select!` — when this branch
                // wins, Tokio drops the handle which sets the task's cancelled
                // flag. For `spawn_blocking` this means the thread will be
                // reclaimed once it returns.
                crate::core::io_health::record_freeze();
                tracing::error!(
                    tool = name,
                    timeout_secs = limit.as_secs(),
                    "tool watchdog fired — handler cancelled to reclaim pool slot"
                );
                Err(ErrorData::internal_error(
                    format!(
                        "tool '{name}' exceeded its {}s watchdog and was abandoned. \
                         The MCP server is still running — retry or narrow the request.",
                        limit.as_secs()
                    ),
                    None,
                ))
            }
        }
    }

    /// Collapse a `spawn_blocking` join result into the handler result.
    /// A `JoinError` (handler panic) becomes a clean error instead of crashing
    /// the request task.
    fn unwrap_join(
        name: &str,
        joined: Result<Result<ToolOutput, ErrorData>, tokio::task::JoinError>,
    ) -> Result<ToolOutput, ErrorData> {
        match joined {
            Ok(inner) => inner,
            Err(join_err) => {
                tracing::error!(
                    tool = name,
                    is_panic = join_err.is_panic(),
                    "tool handler did not complete (panic isolated on the blocking pool)"
                );
                Err(ErrorData::internal_error(
                    format!(
                        "tool '{name}' failed unexpectedly. The MCP server is still running \
                         — retry or use a different approach."
                    ),
                    None,
                ))
            }
        }
    }

    /// Watchdog deadline for a single tool handler, or `None` to disable it.
    ///
    /// `sobre_shell` runs arbitrary user commands (builds, long
    /// test suites) and already enforces its own command timeout, so a generic
    /// watchdog would wrongly abort a legitimate long-running command. Every
    /// other tool is bounded so a hang can never swallow the JSON-RPC response.
    fn handler_watchdog(name: &str) -> Option<Duration> {
        (!super::is_shell_tool_name(name)).then_some(TOOL_WATCHDOG)
    }
}

/// Per-tool watchdog budget. Long enough that no legitimate
/// read/search/graph call ever hits it, short enough that a hang degrades to a
/// clean error instead of a dropped request.
const TOOL_WATCHDOG: Duration = Duration::from_secs(120);

/// The arguments a tool's schema declares as a filesystem path, resolved
/// against the session's roots before the handler runs.
pub(crate) const PATH_LIKE_KEYS: &[&str] = &["path", "to", "project_root"];

#[cfg(test)]
mod tests {
    use super::*;

    /// A call whose absolute path re-roots the session says so.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reroot_is_said_in_the_output() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join(".copilot");
        let real = tmp.path().join("repo");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(real.join(".git")).unwrap();
        std::fs::write(real.join("a.txt"), "ok\n").unwrap();

        let server = SobreServer::new_with_startup(None, None);
        {
            let mut session = server.session.write().await;
            session.project_root = Some(agent.to_string_lossy().to_string());
            session.shell_cwd = Some(agent.to_string_lossy().to_string());
        }
        let args = serde_json::json!({ "path": real.join("a.txt").to_string_lossy() });
        let (text, ..) = server
            .dispatch_tool("sobre_read", args.as_object(), None)
            .await
            .unwrap();
        assert!(text.starts_with("[sobre: project root is now "), "{text}");

        let (again, ..) = server
            .dispatch_tool("sobre_read", args.as_object(), None)
            .await
            .unwrap();
        assert!(!again.contains("project root is now"), "{again}");
    }

    /// Every path key the dispatcher resolves is one some tool declares.
    #[test]
    fn path_like_keys_are_declared_by_some_tool() {
        let registry = crate::server::registry::build_registry();
        for key in PATH_LIKE_KEYS {
            assert!(
                registry.names().iter().any(|name| registry
                    .get_arc(name)
                    .and_then(|t| t
                        .tool_def()
                        .input_schema
                        .get("properties")
                        .and_then(serde_json::Value::as_object)
                        .map(|p| p.contains_key(*key)))
                    .unwrap_or(false)),
                "{key} is declared by no tool"
            );
        }
    }

    #[test]
    fn watchdog_disabled_for_long_running_shell_tools() {
        assert!(
            SobreServer::handler_watchdog("sobre_shell").is_none(),
            "sobre_shell runs arbitrary user commands and must not be watchdog-bounded"
        );
    }

    #[test]
    fn every_other_tool_is_watchdog_bounded() {
        assert_eq!(
            SobreServer::handler_watchdog("sobre_read"),
            Some(TOOL_WATCHDOG)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watchdog_returns_error_on_hung_handler() {
        use std::sync::atomic::{AtomicBool, Ordering};
        // A hung handler must surface as a clean error (not a dropped request),
        // which is the core guarantee. The stop flag lets the simulated
        // hang exit promptly after the assertion so the runtime shuts down fast.
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stop_in = stop.clone();
        let join = tokio::task::spawn_blocking(move || {
            for _ in 0..400 {
                if stop_in.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(ToolOutput::simple("late".to_string()))
        });
        let start = std::time::Instant::now();
        let result =
            SobreServer::watchdog_join("mock", join, Some(Duration::from_millis(200))).await;
        let elapsed = start.elapsed();
        stop.store(true, Ordering::Relaxed);
        assert!(result.is_err(), "hung handler must surface as an error");
        assert!(
            elapsed < Duration::from_secs(2),
            "watchdog must fire promptly, took {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watchdog_passes_through_fast_handler() {
        let join = tokio::task::spawn_blocking(|| Ok(ToolOutput::simple("ok".to_string())));
        let result = SobreServer::watchdog_join("mock", join, Some(Duration::from_secs(5))).await;
        assert_eq!(result.unwrap().text, "ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watchdog_none_awaits_handler_to_completion() {
        let join = tokio::task::spawn_blocking(|| Ok(ToolOutput::simple("ok".to_string())));
        let result = SobreServer::watchdog_join("sobre_shell", join, None).await;
        assert_eq!(result.unwrap().text, "ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handler_panic_becomes_error_not_crash() {
        let join: tokio::task::JoinHandle<Result<ToolOutput, ErrorData>> =
            tokio::task::spawn_blocking(|| panic!("simulated handler panic"));
        let result = SobreServer::watchdog_join("mock", join, Some(Duration::from_secs(5))).await;
        assert!(
            result.is_err(),
            "a handler panic must surface as an error; the server process survives"
        );
    }

    /// A typo'd tool name must return a "did you mean" hint from the
    /// live registry instead of a bare unknown-tool error; a name nowhere
    /// near any registered tool must stay suggestion-free.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_tool_suggests_closest_registered_name() {
        let server = crate::tools::create_server();

        let err = server
            .dispatch_tool("sobre_raed", None, None)
            .await
            .expect_err("typo'd tool must error");
        assert!(
            err.message.contains("Unknown tool: sobre_raed"),
            "unexpected message: {}",
            err.message
        );
        assert!(
            err.message.contains("did you mean 'sobre_read'"),
            "missing suggestion: {}",
            err.message
        );

        let err = server
            .dispatch_tool("zzz_qqq_www", None, None)
            .await
            .expect_err("unrelated name must error");
        assert!(
            !err.message.contains("did you mean"),
            "no confident suggestion for garbage: {}",
            err.message
        );
    }

    /// A tool whose text reports a failure comes back as a failure, so
    /// the client sets `isError` instead of reading an error as a success.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failure_text_marks_the_call_failed() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        let server = crate::tools::create_server();
        let mut args = serde_json::Map::new();
        args.insert("action".into(), serde_json::json!("bogus"));
        let (text, _, outcome, _) = server
            .dispatch_tool("sobre_graph", Some(&args), None)
            .await
            .expect("dispatch");
        assert!(text.starts_with("Unknown action"), "{text}");
        assert_eq!(
            outcome,
            Some(crate::server::tool_trait::ShellOutcome::Failed)
        );

        args.insert("action".into(), serde_json::json!("status"));
        let (_, _, outcome, _) = server
            .dispatch_tool("sobre_graph", Some(&args), None)
            .await
            .expect("dispatch");
        assert_eq!(outcome, None, "a normal answer is not a failure");
    }
}
