//! `LeanCtxServer::call_tool_guarded` — the guarded tool-dispatch path — and
//! root resolution. Split out of `server/mod.rs` to keep that module focused on
//! wiring. `use super::*` re-imports the parent aliases and sibling submodules.

#[allow(clippy::wildcard_imports)]
use super::*;

impl LeanCtxServer {
    pub(crate) async fn call_tool_guarded(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, ErrorData> {
        self.check_idle_expiry().await;
        self.resolve_roots_once().await;

        let tool_name = request.name.as_ref().to_string();
        let tool_args = request.arguments;
        let name = tool_name.as_str();
        let args = tool_args.as_ref();

        // #990: determine machine-readability *before* the once-per-session
        // decorations below. A machine-readable invocation (e.g. ctx_outline
        // format=json) must reach the client byte-exact and parseable, so every
        // prose decoration and terse compression is suppressed and the pure
        // pre-decoration body is restored at the end (see the `machine_readable`
        // guard near the end of this function). Computing it here — not after
        // dispatch — means such a call also never *consumes* a latched
        // once-per-session flag (auto-context briefing, rules tip) whose prose
        // we would then discard, so those surface on the next human-facing call.
        let machine_readable = self
            .registry
            .as_ref()
            .and_then(|r| r.get_arc(name))
            .is_some_and(|tool| tool.produces_machine_readable(args));

        // Skip the session wake-up briefing for machine-readable calls: the
        // pre-hook latches `session_initialized` via compare-exchange, so calling
        // it here would burn the once-per-session slot for a briefing we then
        // throw away. Deferring keeps the briefing intact for the next call.
        // The session-start overview call already returns that briefing's
        // content, so it counts as the briefing: the slot is closed and the
        // project summary is not sent a second time on the next call.
        let auto_context = if machine_readable {
            None
        } else if crate::tools::autonomy::is_session_overview_call(name, args) {
            crate::tools::autonomy::mark_session_briefed(&self.autonomy);
            None
        } else {
            let task = {
                let session = self.session.read().await;
                session.task.as_ref().map(|t| t.description.clone())
            };
            let project_root = {
                let session = self.session.read().await;
                session.project_root.clone()
            };
            let cache_timeout =
                tokio::time::timeout(std::time::Duration::from_secs(5), self.cache.write()).await;
            if let Ok(mut cache) = cache_timeout {
                crate::tools::autonomy::session_lifecycle_pre_hook(
                    &self.autonomy,
                    &mut cache,
                    task.as_deref(),
                    project_root.as_deref(),
                    CrpMode::effective(),
                )
            } else {
                tracing::warn!("pre-dispatch: cache write-lock timeout (5s), skipping autonomy");
                None
            }
        };

        let config = crate::core::config::Config::load_arc();

        let tool_start = std::time::Instant::now();
        let (mut result_text, tool_saved_tokens, shell_outcome, content_blocks) =
            match self.dispatch_tool(name, args).await {
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

        let is_raw_shell = name == "ctx_shell" && {
            let arg_raw = helpers::get_bool(args, "raw").unwrap_or(false);
            let arg_bypass = helpers::get_bool(args, "bypass").unwrap_or(false);
            arg_raw || arg_bypass || crate::core::runtime_flags::raw_enabled()
        };

        let pre_terse_len = result_text.len();
        let output_tokens = crate::core::tokens::count_tokens(&result_text) as u64;

        // Context IR: record lineage for every tool call.
        if let Some(ref ir) = self.context_ir {
            let tool_duration = tool_start.elapsed();
            let source_kind = post_process::context_ir_source_kind(name);
            let ir_path = helpers::get_str(args, "path");
            let ir_command = helpers::get_str(args, "command");
            let ir_mode = helpers::get_str(args, "mode");
            let excerpt = if result_text.len() > 200 {
                let mut end = 200;
                while !result_text.is_char_boundary(end) && end > 0 {
                    end -= 1;
                }
                &result_text[..end]
            } else {
                &result_text
            };
            let input = crate::core::context_ir::RecordIrInput {
                kind: source_kind,
                tool: name,
                client_name: None,
                agent_id: None,
                path: ir_path.as_deref(),
                command: ir_command.as_deref(),
                pattern: ir_mode.as_deref(),
                input_tokens: pre_terse_len / 4,
                output_tokens: output_tokens as usize,
                duration: tool_duration,
                content_excerpt: excerpt,
            };
            ir.write().await.record(input);
        }

        // Re-fetch feedback: re-reads, re-runs and `ctx_expand` mean the
        // compressed form lost something. The session degrade can only lower
        // compression below the configured level, and a change is announced.
        let degrade_note = {
            let mut signals = self.refetch_signals.write().await;
            if name == "ctx_read" {
                let path = helpers::get_str(args, "path").unwrap_or_default();
                let mode = helpers::get_str(args, "mode").unwrap_or_else(|| "auto".into());
                let fresh = helpers::get_bool(args, "fresh").unwrap_or(false);
                signals.record_read(&path, &mode, fresh);
            } else if name == "ctx_shell" {
                let cmd = helpers::get_str(args, "command").unwrap_or_default();
                signals.record_shell(&cmd);
            } else if name == "ctx_expand" {
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
        result_text = post_process::compress_terse(result_text, name, args, &config, is_raw_shell);

        let verify_cfg = crate::core::output_verification::VerificationConfig::default();

        if !is_raw_shell {
            let vr = crate::core::output_verification::verify_output(
                &pre_compression,
                &result_text,
                &verify_cfg,
            );
            if !vr.warnings.is_empty() {
                let msg = format!("[VERIFY] {}", vr.format_compact());
                result_text = format!("{result_text}\n\n{msg}");
            }
        }

        if !is_raw_shell && let Some(ctx) = auto_context {
            result_text = format!("{ctx}\n\n{result_text}");
        }

        if let Some(note) = degrade_note {
            result_text = format!("{result_text}\n\n{note}");
        }

        if name == "ctx_read" {
            let cache_clone = self.cache.clone();
            let autonomy_clone = self.autonomy.clone();
            tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(async {
                    // #807: bounded lock — the old unbounded `.write().await`
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
            // #520 opt-in debug log: a full per-call record (tool, args, result
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

        // OPT-4: dispatch/mod.rs records savings before terse/hints run; this
        // finalizes the real sent-token count and corrects persistent stats.
        let output_token_count = post_process::finalize_token_count_and_adjust(
            name,
            &result_text,
            pre_terse_len,
            output_tokens,
            tool_saved_tokens,
        );

        let action = helpers::get_str(args, "action");

        self.record_receipt_and_cost(
            name,
            args,
            action.as_deref(),
            &result_text,
            output_token_count,
        )
        .await;

        let skip_checkpoint = matches!(name, "ctx_session" | "ctx_knowledge" | "ctx_impact");

        // #1020: tool-calls.log is now written on the dispatch path
        // (record_call_with_path / record_call_with_timing) with the real
        // original/saved/mode and the measured handler duration. The previous
        // zero-filled append here overwrote every row with `orig=0 saved=0 mode=-`.

        // The only place that advances `call_count`.
        if !skip_checkpoint {
            self.increment_call_count();
        }

        // #990: a machine-readable invocation (e.g. ctx_outline format=json) must
        // return a byte-exact, parseable payload. The state-consuming briefings
        // are already skipped above (so their once-per-session flags survive),
        // but other steps still append recomputed prose (verify footer, throttle
        // / budget warning, deprecation notice) or compress the body — all of
        // which break a JSON contract. This guard is the robust catch-all:
        // restore the pure body captured *before* compression and decoration.
        // Redaction + sensitivity were applied earlier so the security envelope
        // is preserved. `ctx_outline` output is never teed, so
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
        let list_result = match peer.list_roots().await {
            Ok(r) => r,
            Err(e) => {
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
        // Indices warm lazily on first use of a tool that needs them (#152) —
        // the dispatch path for this very call handles it via
        // `index_orchestrator::ensure_warm_for_tool`, so no eager scan here.
    }
}

/// Classifies a failed `roots/list` call (GH #694). `-32601 Method not found`
/// means the client declared the roots capability but does not implement the
/// request (Cursor's documented behavior, #699) — retrying can never succeed.
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
/// (GitHub #389): a failing command sets `isError: true` and a
/// `structuredContent` payload (`{"exitCode": N}`), so
/// clients no longer have to regex-parse the `[exit:N]` text footer. The text
/// content is identical in both cases — only the metadata changes. A non-zero
/// exit that is *not* a failure (#1090/#1086) carries neither, because some
/// clients render `structuredContent` in place of the text (#1127).
fn finalize_call_result(
    result_text: &str,
    shell_outcome: Option<crate::server::tool_trait::ShellOutcome>,
) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(result_text.to_owned())]);
    if let Some(outcome) = shell_outcome
        && is_shell_error(outcome, result_text)
    {
        // #1127: clients that understand `structuredContent` render it *instead
        // of* the text block on a non-error result, so attaching it to a benign
        // non-zero exit (exit 1 with output per #1090, exit 124 with partial
        // output per #1086) hid the command output completely — `ls /nope` came
        // back as a bare `{"exitCode":1}`. Only errors, whose text the client
        // surfaces separately, carry the structured payload.
        result.is_error = Some(true);
        result.structured_content = outcome.structured();
    }
    result
}

/// #1090: exit 1 with non-empty stdout is not an error for grep/diff/test.
/// #1086/#1089: exit 124 (timeout) with captured partial output is returned
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
        crate::server::tool_trait::ShellOutcome::Exit(_) => true,
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
        // #1090: exit 1 with command output (before [exit:] footer) is NOT
        // a tool error — grep/diff/test exit 1 with results is normal.
        let r = finalize_call_result("boom\n[exit:1]", Some(ShellOutcome::Exit(1)));
        assert_ne!(
            r.is_error,
            Some(true),
            "exit 1 with output must NOT set isError (#1090)"
        );
        // #1127: and it must carry no structuredContent either — clients that
        // prefer it would render `{"exitCode":1}` and drop the output.
        assert!(
            r.structured_content.is_none(),
            "benign exit 1 must not shadow its output with structuredContent (#1127)"
        );
        assert_eq!(text_of(&r), "boom\n[exit:1]", "output text is preserved");
    }

    #[test]
    fn stderr_only_failure_keeps_its_text() {
        // #1127: `ls /nonexistent` writes only to stderr and exits 1. The text
        // block is the sole carrier of the diagnostic, so it must survive.
        let text = "ls: /nonexistent-path-xyz: No such file or directory\n[exit:1]";
        let r = finalize_call_result(text, Some(ShellOutcome::Exit(1)));
        assert_ne!(r.is_error, Some(true));
        assert!(r.structured_content.is_none());
        assert_eq!(text_of(&r), text);
    }

    #[test]
    fn timeout_with_partial_output_keeps_its_text() {
        // #1086/#1127: same shape for exit 124 — partial output is a success
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
        assert_eq!(
            r.is_error,
            Some(true),
            "exit >= 2 must always set isError (#389)"
        );
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

    /// Cursor's pattern (#699): roots capability declared, `roots/list`
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

    /// The VS Code multi-window pattern (GH #694): the second window's client
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
