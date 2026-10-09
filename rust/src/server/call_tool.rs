//! `SobreServer::call_tool_guarded` — the guarded tool-dispatch path — and
//! root resolution. Split out of `server/mod.rs` to keep that module focused on
//! wiring. `use super::*` re-imports the parent aliases and sibling submodules.

#[allow(clippy::wildcard_imports)]
use super::*;

/// How long the first tool call waits for the client's `roots/list` answer.
const ROOTS_LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl SobreServer {
    pub(crate) async fn call_tool_guarded(
        &self,
        request: CallToolRequestParams,
        progress: Option<crate::server::progress::ProgressSender>,
    ) -> Result<CallToolResult, ErrorData> {
        self.check_idle_expiry().await;
        self.resolve_roots_once().await;

        let tool_name = request.name.as_ref().to_string();
        let tool_args = request.arguments;
        let name = tool_name.as_str();
        let args = tool_args.as_ref();

        // A machine-readable invocation (e.g. sobre_outline format=json)
        // must reach the client byte-exact and parseable, so every prose
        // decoration and terse compression is suppressed and the pure
        // pre-decoration body is restored at the end (see the
        // `machine_readable` guard near the end of this function).
        let machine_readable = self
            .registry
            .get_arc(name)
            .is_some_and(|tool| tool.produces_machine_readable(args));

        let config = crate::core::config::Config::load_arc();

        let tool_start = std::time::Instant::now();
        let (mut result_text, tool_saved_tokens, shell_outcome, content_blocks) =
            match self.dispatch_tool(name, args, progress).await {
                Ok(tuple) => tuple,
                Err(e) => {
                    crate::core::debug_log::log_mcp_error(name, args, &format!("{e:?}"));
                    return Err(e);
                }
            };

        // Image/binary content blocks: skip all post-processing, return directly.
        if let Some(blocks) = content_blocks {
            let mut result = CallToolResult::success(blocks);
            if let Some(outcome) = shell_outcome
                && outcome.is_error()
            {
                result.is_error = Some(true);
            }
            return Ok(result);
        }

        let pre_terse_len = result_text.len();
        let output_tokens = crate::core::tokens::count_tokens(&result_text) as u64;

        // Re-fetch feedback: re-reads, re-runs and `sobre_expand` mean the
        // compressed form lost something. The session degrade can only lower
        // compression below the configured level, and a change is announced.
        let degrade_note = {
            let mut signals = self.refetch_signals.write().await;
            if name == "sobre_read" {
                let path = helpers::get_str(args, "path").unwrap_or_default();
                let mode = helpers::get_str(args, "mode").unwrap_or_else(|| "auto".into());
                let fresh = helpers::get_bool(args, "fresh").unwrap_or(false);
                signals.record_read(&path, &mode, fresh);
            } else if name == "sobre_shell" {
                let cmd = helpers::get_str(args, "command").unwrap_or_default();
                signals.record_shell(&cmd);
            } else if name == "sobre_expand" {
                signals.record_retrieve();
            }
            use crate::core::config::CompressionLevel;
            let changed = CompressionLevel::apply_degrade_action(
                CompressionLevel::degrade_action(
                    signals.correction_count(),
                    signals.retrieve_count(),
                ),
                &config,
            );
            signals.prune();
            changed.map(|level| {
                format!(
                    "[sobre: compression is now {} for this session — {}]",
                    level.label(),
                    if level == CompressionLevel::effective_configured(&config) {
                        "back to the configured level"
                    } else {
                        "lowered after repeated re-reads, re-runs or expands"
                    }
                )
            })
        };

        let pre_compression = result_text.clone();
        result_text = post_process::compress_terse(result_text, name, &config);

        // A condensed text that lost a path, an identifier or structure is
        // not sent: the agent gets the tool's own output instead.
        if result_text != pre_compression
            && !crate::core::output_verification::verify_output(&pre_compression, &result_text)
                .warnings
                .is_empty()
        {
            result_text.clone_from(&pre_compression);
        }

        if let Some(note) = degrade_note {
            result_text = format!("{result_text}\n\n{note}");
        }

        if name == "sobre_read" {
            let cache_clone = self.cache.clone();
            let autonomy_clone = self.autonomy.clone();
            tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(async {
                    // Bounded lock — the old unbounded `.write.await`
                    // could queue behind a long computation and then hold the
                    // lock during dedup, cascading the stall.
                    let cache_timeout = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        cache_clone.write(),
                    )
                    .await;
                    if let Ok(mut cache) = cache_timeout {
                        crate::tools::autonomy::maybe_auto_dedup(&autonomy_clone, &mut cache);
                    } else {
                        tracing::debug!("background auto_dedup: cache lock timeout (5s), skipping");
                    }
                })
                .catch_unwind()
                .await;
                if let Err(e) = result {
                    let msg = e
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| e.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown");
                    tracing::error!("background auto_dedup panicked: {msg}");
                }
            });
        }

        {
            let tool_name = name.to_string();
            let summary = result_text.lines().next().unwrap_or("").to_string();
            // Opt-in debug log: a full per-call record (tool, args, result
            // preview, savings, wall time). Captured here and written off the hot
            // path in the existing journal thread; no-op unless `debug_log` is on.
            let dbg_args = args.cloned();
            let dbg_bytes = result_text.len();
            let dbg_saved = tool_saved_tokens;
            let dbg_elapsed = tool_start.elapsed();
            std::thread::spawn(move || {
                crate::core::debug_log::log_mcp_call(
                    &tool_name,
                    dbg_args.as_ref(),
                    &summary,
                    dbg_bytes,
                    dbg_saved,
                    dbg_elapsed,
                );
            });
        }

        let output_token_count =
            post_process::final_token_count(&result_text, pre_terse_len, output_tokens);
        // The tool's own savings plus whatever terse removed afterwards: the
        // original is the pre-terse count plus what the tool already saved.
        crate::core::usage::record_compression(
            crate::core::usage::Surface::Mcp,
            name,
            usize::try_from(output_tokens).unwrap_or(usize::MAX) + tool_saved_tokens,
            output_token_count,
        );

        let action = helpers::get_str(args, "action");

        self.record_receipt(name, args, action.as_deref(), &result_text)
            .await;

        let skip_checkpoint = matches!(name, "sobre_session" | "sobre_knowledge");

        // The only place that advances `call_count`.
        if !skip_checkpoint {
            self.increment_call_count();
        }

        // A machine-readable invocation (e.g. sobre_outline format=json) must
        // return a byte-exact, parseable payload. The state-consuming briefings
        // are already skipped above (so their once-per-session flags survive),
        // but other steps still append recomputed prose (verify footer, throttle
        // / budget warning) or compress the body — all of
        // which break a JSON contract. This guard is the robust catch-all:
        // restore the pure body captured *before* compression and decoration.
        // Redaction + sensitivity were applied earlier so the security envelope
        // is preserved. `sobre_outline` output is never teed, so
        // `pre_compression` is the unmodified body here; no-op otherwise.
        if machine_readable {
            result_text = pre_compression;
        }

        Ok(finalize_call_result(&result_text, shell_outcome))
    }

    /// Resolve project root from MCP client roots (once per session).
    /// Called on the first tool call. If the client supports `roots/list`,
    /// we query it and pick the best root with project markers.
    ///
    /// Roots is SEP-2577-deprecated in rmcp 2.0 but still fully functional; we
    /// keep it for client-driven project-root auto-detection until MCP removes it.
    #[expect(deprecated)]
    async fn resolve_roots_once(&self) {
        use std::sync::atomic::Ordering;
        if !self.has_client_roots.load(Ordering::Relaxed) {
            return;
        }
        if self.roots_resolved.swap(true, Ordering::Relaxed) {
            return;
        }
        let peer_guard = self.peer.read().await;
        let Some(peer) = peer_guard.as_ref() else {
            return;
        };
        // A client that declares roots but never answers must not hold the
        // first tool call: past the deadline, the session keeps its root.
        let list_result = match tokio::time::timeout(ROOTS_LIST_TIMEOUT, peer.list_roots()).await {
            Err(_) => {
                tracing::warn!(
                    "roots/list: no answer within {}s — giving up",
                    ROOTS_LIST_TIMEOUT.as_secs()
                );
                return;
            }
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let permanent = roots_list_failure_is_permanent(&e);
                const MAX_ATTEMPTS: u32 = 3;
                let attempts = self.roots_list_attempts.fetch_add(1, Ordering::Relaxed) + 1;
                if !permanent && attempts < MAX_ATTEMPTS {
                    self.roots_resolved.store(false, Ordering::Relaxed);
                }
                tracing::warn!(
                    "roots/list failed (attempt {attempts}, {}): {e}",
                    if permanent {
                        "client does not implement it — giving up"
                    } else if attempts < MAX_ATTEMPTS {
                        "will retry on a later tool call"
                    } else {
                        "retry budget exhausted"
                    }
                );
                return;
            }
        };
        drop(peer_guard);

        let uris: Vec<String> = list_result.roots.iter().map(|r| r.uri.clone()).collect();
        let validated_paths = roots::valid_dir_paths_from_uris(&uris);
        let Some(new_root) = roots::best_root_from_uris(&uris) else {
            return;
        };
        // Defense-in-depth: never adopt a broad/unsafe root (HOME, `/`, agent
        // sandbox dirs) even if the client reports it — it would pollute the
        // session and resolve relative paths against the wrong tree.
        if crate::core::pathutil::is_broad_or_unsafe_root(std::path::Path::new(&new_root)) {
            tracing::warn!("MCP roots: ignoring unsafe project root {new_root}");
            return;
        }

        let mut session = self.session.write().await;
        let old_root = session.project_root.clone();

        let other_roots: Vec<String> = validated_paths
            .iter()
            .filter(|p| p.as_str() != new_root)
            .cloned()
            .collect();
        if !other_roots.is_empty() {
            session.extra_roots = other_roots;
            tracing::info!(
                "MCP roots: {} extra root(s) registered",
                session.extra_roots.len()
            );
        }

        if old_root.as_deref() == Some(&new_root) {
            let _ = session.save();
            return;
        }
        tracing::info!(
            "MCP roots: switching project root from {:?} to {new_root}",
            old_root
        );
        if let Some(existing) =
            crate::core::session::SessionState::load_latest_for_project_root(&new_root)
        {
            *session = existing;
            session.extra_roots = validated_paths
                .iter()
                .filter(|p| p.as_str() != new_root)
                .cloned()
                .collect();
        }
        session.project_root = Some(new_root);
        let _ = session.save();
        drop(session);
        // Indices warm lazily on first use of a tool that needs them —
        // the dispatch path for this very call handles it via
        // `index_orchestrator::ensure_warm_for_tool`, so no eager scan here.
    }
}

/// Classifies a failed `roots/list` call. `-32601 Method not found`
/// means the client declared the roots capability but does not implement the
/// request (Cursor's documented behavior) — retrying can never succeed.
/// Everything else (timeout, transport hiccup while an IDE window is still
/// starting up) is transient and worth a bounded retry on a later tool call.
fn roots_list_failure_is_permanent(e: &rmcp::ServiceError) -> bool {
    matches!(
        e,
        rmcp::ServiceError::McpError(mcp)
            if mcp.code == rmcp::model::ErrorCode::METHOD_NOT_FOUND
    )
}

/// Build the final `CallToolResult`, surfacing shell failures in MCP metadata
///: a failing command sets `isError: true` and a
/// `structuredContent` payload (`{"exitCode": N}`), so
/// clients no longer have to regex-parse the `[exit:N]` text footer. The text
/// content is identical in both cases — only the metadata changes. A non-zero
/// exit that is *not* a failure carries neither, because some
/// clients render `structuredContent` in place of the text.
fn finalize_call_result(
    result_text: &str,
    shell_outcome: Option<crate::server::tool_trait::ShellOutcome>,
) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(result_text.to_owned())]);
    if let Some(outcome) = shell_outcome
        && is_shell_error(outcome, result_text)
    {
        // Clients that understand `structuredContent` render it *instead
        // of* the text block on a non-error result, so attaching it to a benign
        // non-zero exit (exit 1 with output, exit 124 with partial
        // output) hid the command output completely — `ls /nope` came
        // back as a bare `{"exitCode":1}`. Only errors, whose text the client
        // surfaces separately, carry the structured payload.
        result.is_error = Some(true);
        result.structured_content = outcome.structured();
    }
    result
}

/// Exit 1 with non-empty stdout is not an error for grep/diff/test.
/// Exit 124 (timeout) with captured partial output is returned
/// as a success with a timeout marker, not as an error. The partial output is
/// often enough to answer the question; treating it as a failure causes the
/// client to retry the entire pipeline.
fn is_shell_error(outcome: crate::server::tool_trait::ShellOutcome, output: &str) -> bool {
    match outcome {
        crate::server::tool_trait::ShellOutcome::Exit(0) => false,
        crate::server::tool_trait::ShellOutcome::Exit(1) => {
            output.trim().is_empty() || output.trim().starts_with("[exit:")
        }
        crate::server::tool_trait::ShellOutcome::Exit(124) => {
            // Timeout with partial output: return as success so the client
            // sees the captured data. The ERROR marker in the text body still
            // signals the timeout to the agent.
            crate::server::execute::output_before_timeout_marker(output).is_none_or(str::is_empty)
        }
        crate::server::tool_trait::ShellOutcome::Exit(_)
        | crate::server::tool_trait::ShellOutcome::Failed => true,
    }
}

#[cfg(test)]
mod shell_outcome_tests {
    use super::*;
    use crate::server::tool_trait::ShellOutcome;

    fn text_of(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    #[test]
    fn a_failed_tool_sets_is_error_without_structured_content() {
        let r = finalize_call_result("ERROR: path is required", Some(ShellOutcome::Failed));
        assert_eq!(r.is_error, Some(true));
        assert!(r.structured_content.is_none());
        assert_eq!(text_of(&r), "ERROR: path is required");
    }

    #[test]
    fn success_exit_is_not_an_error() {
        let r = finalize_call_result("ok", Some(ShellOutcome::Exit(0)));
        assert_ne!(r.is_error, Some(true), "exit 0 must not set isError");
        assert!(
            r.structured_content.is_none(),
            "happy path stays token-neutral: no structuredContent on exit 0"
        );
        assert_eq!(text_of(&r), "ok");
    }

    #[test]
    fn nonzero_exit_with_output_is_not_error() {
        // Exit 1 with command output (before [exit:] footer) is NOT
        // a tool error — grep/diff/test exit 1 with results is normal.
        let r = finalize_call_result("boom\n[exit:1]", Some(ShellOutcome::Exit(1)));
        assert_ne!(
            r.is_error,
            Some(true),
            "exit 1 with output must NOT set isError"
        );
        // And it must carry no structuredContent either — clients that
        // prefer it would render `{"exitCode":1}` and drop the output.
        assert!(
            r.structured_content.is_none(),
            "benign exit 1 must not shadow its output with structuredContent"
        );
        assert_eq!(text_of(&r), "boom\n[exit:1]", "output text is preserved");
    }

    #[test]
    fn stderr_only_failure_keeps_its_text() {
        // `ls /nonexistent` writes only to stderr and exits 1. The text
        // block is the sole carrier of the diagnostic, so it must survive.
        let text = "ls: /nonexistent-path-xyz: No such file or directory\n[exit:1]";
        let r = finalize_call_result(text, Some(ShellOutcome::Exit(1)));
        assert_ne!(r.is_error, Some(true));
        assert!(r.structured_content.is_none());
        assert_eq!(text_of(&r), text);
    }

    #[test]
    fn timeout_with_partial_output_keeps_its_text() {
        // Same shape for exit 124 — partial output is a success
        // result, so structuredContent must not displace it.
        let text = "line one\nERROR: command timed out after 5000ms";
        let r = finalize_call_result(text, Some(ShellOutcome::Exit(124)));
        assert_ne!(r.is_error, Some(true));
        assert!(r.structured_content.is_none());
        assert_eq!(text_of(&r), text);
    }

    #[test]
    fn exit_1_without_output_is_error() {
        // Exit 1 with only the [exit:] footer (no command output) IS an error.
        let r = finalize_call_result("[exit:1]", Some(ShellOutcome::Exit(1)));
        assert_eq!(
            r.is_error,
            Some(true),
            "exit 1 with no command output must set isError"
        );
    }

    #[test]
    fn exit_2_is_always_error() {
        let r = finalize_call_result("error output", Some(ShellOutcome::Exit(2)));
        assert_eq!(r.is_error, Some(true), "exit >= 2 must always set isError");
        assert_eq!(
            r.structured_content,
            Some(serde_json::json!({ "exitCode": 2 })),
            "guards must be able to read exitCode without text parsing"
        );
    }

    #[test]
    fn negative_exit_codes_are_reported() {
        // Signal terminations are mapped to negative/128+n codes by execute();
        // whatever the value, non-zero must surface as an error.
        let r = finalize_call_result("killed", Some(ShellOutcome::Exit(-1)));
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            r.structured_content,
            Some(serde_json::json!({ "exitCode": -1 }))
        );
    }

    #[test]
    fn non_shell_tools_are_unaffected() {
        let r = finalize_call_result("file contents", None);
        assert_ne!(r.is_error, Some(true));
        assert!(r.structured_content.is_none());
    }
}

#[cfg(test)]
mod roots_retry_tests {
    use super::roots_list_failure_is_permanent;

    /// Cursor's pattern: roots capability declared, `roots/list`
    /// answered with `-32601` — retrying is pointless and must stop.
    #[test]
    fn method_not_found_is_permanent() {
        let err = rmcp::ServiceError::McpError(rmcp::model::ErrorData::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            "Method not found",
            None,
        ));
        assert!(roots_list_failure_is_permanent(&err));
    }

    /// The VS Code multi-window pattern: the second window's client
    /// is still starting up, `roots/list` times out or the transport hiccups —
    /// these must stay retryable so root detection recovers.
    #[test]
    fn timeouts_and_other_mcp_errors_are_transient() {
        let timeout = rmcp::ServiceError::Timeout {
            timeout: std::time::Duration::from_secs(5),
        };
        assert!(!roots_list_failure_is_permanent(&timeout));

        let internal = rmcp::ServiceError::McpError(rmcp::model::ErrorData::new(
            rmcp::model::ErrorCode::INTERNAL_ERROR,
            "boom",
            None,
        ));
        assert!(!roots_list_failure_is_permanent(&internal));
    }
}
