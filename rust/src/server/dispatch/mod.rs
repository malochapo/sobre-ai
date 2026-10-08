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
    /// shell-executing tools so the caller can populate MCP error metadata
    /// (#389). content_blocks carries image/binary MCP blocks when present.
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
        if let Some(tool) = self.registry.as_ref().and_then(|r| r.get_arc(name)) {
            let empty = serde_json::Map::new();
            let args_map = args.unwrap_or(&empty);
            let project_root = {
                let session = self.session.read().await;
                session.project_root.clone().unwrap_or_default()
            };

            // Lazy, demand-driven index warming (#152): only tools that actually
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

            let mut resolved_paths = std::collections::HashMap::new();
            let mut path_errors: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for key in PATH_LIKE_KEYS {
                if let Some(val) = args_map.get(*key) {
                    if let Some(raw) = val.as_str() {
                        match self.resolve_path(raw).await {
                            Ok(resolved) => {
                                if !["path", "project_root", "root"].contains(key) {
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

            let crp_mode = crate::tools::CrpMode::effective();
            let (extra_roots, shell_cwd) = {
                let session = self.session.read().await;
                (session.extra_roots.clone(), session.shell_cwd.clone())
            };
            let ctx = crate::server::tool_trait::ToolContext {
                project_root,
                shell_cwd,
                extra_roots,
                resolved_paths,
                crp_mode,
                cache: Some(self.cache.clone()),
                session: Some(self.session.clone()),
                path_errors,
                bm25_cache: Some(self.bm25_cache.clone()),
                progress_sender: progress,
            };
            // Run the (synchronous) handler on the dedicated blocking pool under
            // a watchdog deadline (#271). `block_in_place` would pin one of the
            // few core workers and — being synchronous — cannot be interrupted
            // by `tokio::time::timeout` from the same task, so a hung handler
            // would silently swallow the JSON-RPC response and the MCP client
            // would crash with "Cannot read properties of undefined (reading
            // 'invoke')". `spawn_blocking` keeps the core workers free and lets
            // the watchdog always return a response.
            let output = self.run_tool_handler(name, tool, args_map, ctx).await?;

            // Image/binary content blocks bypass all text processing.
            if output.content_blocks.is_some() {
                self.record_call(name, 0, 0).await;
                return Ok((
                    String::new(),
                    0,
                    output.shell_outcome,
                    output.content_blocks,
                ));
            }

            if let Some(ref path) = output.path {
                {
                    // Skip ledger record for ctx_read — it's recorded in post_dispatch
                    // with correct final token counts after terse compression.
                    if name != "ctx_read" {
                        let sent_tokens = if output.original_tokens > 0 {
                            output.original_tokens.saturating_sub(output.saved_tokens)
                        } else {
                            crate::core::tokens::count_tokens(&output.text)
                        };
                        let orig = if output.original_tokens > 0 {
                            output.original_tokens
                        } else {
                            sent_tokens
                        };
                        let mode_str = output.mode.as_deref().unwrap_or("full");
                        let mut ledger = self.ledger.write().await;
                        ledger.record(path, mode_str, orig, sent_tokens);
                        ledger.save_debounced();
                    }
                }
            }
            self.record_call(name, output.original_tokens, output.saved_tokens)
                .await;

            let saved = output.saved_tokens;
            let raw_text = output.text;
            return Ok((raw_text, saved, output.shell_outcome, None));
        }

        // Unknown tool (#712): suggest the closest registered name so a typo
        // (`ctx_raed`) costs the agent a hint instead of a blind retry.
        let suggestion = self
            .registry
            .as_ref()
            .and_then(|r| crate::core::levenshtein::closest(name, r.names()))
            .map(|s| format!(" — did you mean '{s}'?"))
            .unwrap_or_default();
        Err(ErrorData::invalid_params(
            format!("Unknown tool: {name}{suggestion}"),
            None,
        ))
    }

    /// Execute a (synchronous) tool handler on the blocking pool under a
    /// watchdog deadline (#271).
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
    /// slot (#1018). A clean error is always returned so the MCP client gets a
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
                // reclaimed once it returns (#1018).
                crate::core::io_health::record_freeze();
                tracing::error!(
                    tool = name,
                    timeout_secs = limit.as_secs(),
                    "tool watchdog fired — handler cancelled to reclaim pool slot (#1018)"
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
    /// `ctx_shell` runs arbitrary user commands (builds, long
    /// test suites) and already enforces its own command timeout, so a generic
    /// watchdog would wrongly abort a legitimate long-running command. Every
    /// other tool is bounded so a hang can never swallow the JSON-RPC response.
    fn handler_watchdog(name: &str) -> Option<Duration> {
        if super::is_shell_tool_name(name) {
            return None;
        }
        watchdog_from_secs(read_watchdog_secs())
    }
}

/// The watchdog budget in seconds. Kept separate from the policy so the pure
/// `secs -> Option<Duration>` mapping stays trivially testable.
fn read_watchdog_secs() -> u64 {
    DEFAULT_TOOL_TIMEOUT_SECS
}

/// Map a watchdog budget in seconds to a duration; `0` disables the watchdog.
fn watchdog_from_secs(secs: u64) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Default per-tool watchdog budget (#271). Long enough that no legitimate
/// read/search/graph call ever hits it, short enough that a hang degrades to a
/// clean error instead of a dropped request.
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 120;

const PATH_LIKE_KEYS: &[&str] = &[
    "path",
    "project_root",
    "root",
    "file",
    "directory",
    "dir",
    "target",
    "source",
    "destination",
    "old_path",
    "new_path",
    "file_path",
    "from",
    "to",
    "base_path",
    "config_path",
    "output",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_like_keys_includes_primary_keys() {
        for primary in &["path", "project_root", "root"] {
            assert!(
                PATH_LIKE_KEYS.contains(primary),
                "primary key '{primary}' missing from PATH_LIKE_KEYS"
            );
        }
    }

    #[test]
    fn watchdog_disabled_for_long_running_shell_tools() {
        assert!(
            SobreServer::handler_watchdog("ctx_shell").is_none(),
            "ctx_shell runs arbitrary user commands and must not be watchdog-bounded"
        );
    }

    #[test]
    fn watchdog_from_secs_zero_disables() {
        assert!(watchdog_from_secs(0).is_none());
    }

    #[test]
    fn watchdog_from_secs_positive_maps_to_duration() {
        assert_eq!(watchdog_from_secs(5).unwrap().as_secs(), 5);
        assert_eq!(
            watchdog_from_secs(DEFAULT_TOOL_TIMEOUT_SECS)
                .unwrap()
                .as_secs(),
            120
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watchdog_returns_error_on_hung_handler() {
        use std::sync::atomic::{AtomicBool, Ordering};
        // A hung handler must surface as a clean error (not a dropped request),
        // which is the core #271 guarantee. The stop flag lets the simulated
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
        let result = SobreServer::watchdog_join("ctx_shell", join, None).await;
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

    /// #712: a typo'd tool name must return a "did you mean" hint from the
    /// live registry instead of a bare unknown-tool error; a name nowhere
    /// near any registered tool must stay suggestion-free.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_tool_suggests_closest_registered_name() {
        let server = crate::tools::create_server();

        let err = server
            .dispatch_tool("ctx_raed", None, None)
            .await
            .expect_err("typo'd tool must error");
        assert!(
            err.message.contains("Unknown tool: ctx_raed"),
            "unexpected message: {}",
            err.message
        );
        assert!(
            err.message.contains("did you mean 'ctx_read'"),
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
}
