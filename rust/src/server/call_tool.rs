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
        elicitation::increment_call();

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

        let args_fp = args
            .map(|a| {
                crate::core::loop_detection::LoopDetector::fingerprint(&serde_json::Value::Object(
                    a.clone(),
                ))
            })
            .unwrap_or_default();
        let throttle_result = {
            let fp = &args_fp;
            let detector_timeout = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                self.loop_detector.write(),
            )
            .await;
            if let Ok(mut detector) = detector_timeout {
                let is_search = crate::core::loop_detection::LoopDetector::is_search_tool(name);
                let is_search_shell = name == "ctx_shell" && {
                    let cmd = args
                        .as_ref()
                        .and_then(|a| a.get("command"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    crate::core::loop_detection::LoopDetector::is_search_shell_command(cmd)
                };

                if is_search || is_search_shell {
                    let search_pattern = args.and_then(|a| {
                        a.get("pattern")
                            .or_else(|| a.get("query"))
                            .and_then(|v| v.as_str())
                    });
                    let shell_pattern = if is_search_shell {
                        args.and_then(|a| a.get("command"))
                            .and_then(|v| v.as_str())
                            .and_then(helpers::extract_search_pattern_from_command)
                    } else {
                        None
                    };
                    let pat = search_pattern.or(shell_pattern.as_deref());
                    detector.record_search(name, fp, pat)
                } else {
                    detector.record_call(name, fp)
                }
            } else {
                tracing::warn!("pre-dispatch: loop_detector write-lock timeout (3s), skipping");
                crate::core::loop_detection::ThrottleResult::default()
            }
        };

        if throttle_result.level == crate::core::loop_detection::ThrottleLevel::Blocked {
            let msg = throttle_result.message.unwrap_or_default();
            return Ok(CallToolResult::success(vec![ContentBlock::text(msg)]));
        }

        let throttle_warning =
            if throttle_result.level == crate::core::loop_detection::ThrottleLevel::Reduced {
                throttle_result.message.clone()
            } else {
                None
            };

        let config = crate::core::config::Config::load_arc();
        let minimal = config.minimal_overhead_effective();

        let tool_start = std::time::Instant::now();
        let (mut result_text, tool_saved_tokens, shell_outcome, content_blocks) =
            match self.dispatch_tool(name, args, minimal).await {
                Ok(tuple) => tuple,
                Err(e) => {
                    if let Ok(mut detector) = tokio::time::timeout(
                        std::time::Duration::from_secs(1),
                        self.loop_detector.write(),
                    )
                    .await
                    {
                        detector.record_error_outcome(name, &args_fp);
                    }
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

        let inline_shell = name == "ctx_shell"
            && crate::core::firewall::should_inline_shell(
                helpers::get_bool(args, "inline").unwrap_or(false),
                result_text.len(),
            );
        let is_raw_shell = name == "ctx_shell" && {
            let arg_raw = helpers::get_bool(args, "raw").unwrap_or(false);
            let arg_bypass = helpers::get_bool(args, "bypass").unwrap_or(false);
            arg_raw || arg_bypass || crate::core::runtime_flags::raw_enabled() || inline_shell
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

        // Correction-loop detection: track re-reads and re-runs as quality signals.
        {
            let mut detector = self.loop_detector.write().await;
            if name == "ctx_read" {
                let path = helpers::get_str(args, "path").unwrap_or_default();
                let mode = helpers::get_str(args, "mode").unwrap_or_else(|| "auto".into());
                let fresh = helpers::get_bool(args, "fresh").unwrap_or(false);
                detector.record_read_for_correction(&path, &mode, fresh);
            } else if name == "ctx_shell" {
                let cmd = helpers::get_str(args, "command").unwrap_or_default();
                detector.record_shell_for_correction(&cmd);
            } else if name == "ctx_expand" || name == "ctx_retrieve" {
                // CCR-learning (#941): a verbatim/original re-fetch means the inline
                // compressed form was too lossy for this session.
                detector.record_retrieve();
            }
            let correction_count = detector.correction_count();
            let retrieve_count = detector.retrieve_count();
            // Auto-degrade: reduce compression when the agent keeps re-fetching what
            // we squeezed out. Correction loops (re-reads/re-runs) and CCR retrieves
            // (ctx_expand/ctx_retrieve) are two views of the same "too aggressive"
            // signal; degrade on the stronger of the two and clear only when neither
            // fires. The level is server state, never part of any output body (#498).
            use crate::core::config::CompressionLevel;
            CompressionLevel::apply_degrade_action(CompressionLevel::degrade_action(
                correction_count,
                retrieve_count,
            ));
            detector.prune_corrections();
        }

        // #212 — per-item sensitivity floor. Enforced uniformly here (before
        // archiving + compression) so it covers both the inline result and the
        // out-of-band copy. No-op unless `sensitivity.enabled` (default off).
        {
            let path_hint = helpers::get_str(args, "path");
            let enforced = crate::core::sensitivity::enforce_text(
                std::mem::take(&mut result_text),
                path_hint.as_deref().map(std::path::Path::new),
                &config.sensitivity_effective(),
            );
            result_text = enforced.into_text();
        }

        // Out-of-band archive + optional context firewall for large tool outputs.
        // For firewallable tools (ctx_shell/ctx_execute/ctx_search/ctx_tree) whose output
        // exceeds the ephemeral threshold, the full (redacted) body is stored out-of-band
        // and the inline result is replaced by a compact digest + ctx_expand drilldown.
        let mut firewalled = false;
        if !(minimal || is_raw_shell) {
            use crate::core::archive;
            let archivable = matches!(
                name,
                "ctx_shell" | "ctx_read" | "ctx_execute" | "ctx_search" | "ctx_tree"
            );
            if archivable && archive::should_archive(&result_text) {
                let cmd = helpers::get_str(args, "command")
                    .or_else(|| helpers::get_str(args, "path"))
                    .unwrap_or_default();
                let session_id = self.session.read().await.id.clone();
                let to_store = crate::core::redaction::redact_text_if_enabled(&result_text);
                let tokens = crate::core::tokens::count_tokens(&to_store);
                if let Some(id) = archive::store(name, &cmd, &to_store, Some(&session_id))
                    && crate::core::firewall::should_firewall(name, tokens)
                {
                    result_text = crate::core::firewall::summarize(&to_store, &id, name, tokens);
                    firewalled = true;
                }
            }
        }

        let pre_compression = result_text.clone();
        // A firewalled result is already a compact digest — re-compressing it would mangle
        // the retrieval instructions for no benefit.
        if !firewalled {
            result_text =
                post_process::compress_terse(result_text, name, args, &config, is_raw_shell);
        }

        // Snapshot BEFORE any decoration (auto-context prefix, throttle/budget
        // warnings, hints): auto-findings must parse the clean tool output, or
        // the injected "--- AUTO CONTEXT ---" header itself becomes a junk
        // finding ("Read ---") that pollutes the session, the knowledge store,
        // and every subsequent wakeup briefing (#658).
        let findings_source = result_text.clone();

        let verify_cfg = crate::core::output_verification::VerificationConfig::default();

        if !is_raw_shell && !firewalled {
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

        if let Some(warning) = throttle_warning {
            result_text = format!("{result_text}\n\n{warning}");
        }

        // Gated on `!machine_readable` (short-circuits before the swap) so a
        // json-first call does not consume this once-per-session slot for a tip
        // we would immediately discard; it then surfaces on the next call.
        if !machine_readable
            && !self
                .rules_stale_checked
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let client = self.client_name.read().await.clone();
            if !client.is_empty() && crate::rules_inject::check_rules_freshness(&client).is_some() {
                // Self-heal: auto-refresh the rules on disk instead of asking
                // the user to run setup manually (#2365). The rewrite is
                // idempotent and cheap; run it off the async runtime.
                let _ = tokio::task::spawn_blocking(|| {
                    if let Some(home) = dirs::home_dir() {
                        let _ = crate::rules_inject::inject_all_rules(&home);
                    }
                })
                .await;
                result_text = format!(
                    "{result_text}\n\n[RULES AUTO-UPDATED] Your lean-ctx rules were written by \
                     an older version and have been refreshed on disk. Start a new session to \
                     load them for full compatibility."
                );
            } else if !self
                .rules_tip_shown
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                if !crate::rules_inject::rules_installed() {
                    result_text = format!(
                        "{result_text}\n\n\
                         --- tip: run 'lean-ctx init' to configure agent rules for optimal AI integration ---"
                    );
                }
            }
        }

        if name == "ctx_read" {
            if minimal {
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
                            tracing::debug!(
                                "background auto_dedup: cache lock timeout (5s), skipping"
                            );
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
            } else {
                let read_path = self
                    .resolve_path_or_passthrough(
                        &helpers::get_str(args, "path").unwrap_or_default(),
                    )
                    .await;
                let project_root = {
                    let session = self.session.read().await;
                    session.project_root.clone()
                };

                // Bounded cache lock for enrichment — degrade gracefully under contention
                let enrich_timeout =
                    tokio::time::timeout(std::time::Duration::from_secs(3), self.cache.write())
                        .await;
                if let Ok(mut cache) = enrich_timeout {
                    let enrich = crate::tools::autonomy::enrich_after_read(
                        &self.autonomy,
                        &mut cache,
                        &read_path,
                        project_root.as_deref(),
                        None,
                        crate::tools::CrpMode::effective(),
                        false,
                    );
                    if let Some(hint) = enrich.related_hint {
                        result_text = format!("{result_text}\n{hint}");
                    }
                    crate::tools::autonomy::maybe_auto_dedup(&self.autonomy, &mut cache);
                } else {
                    tracing::warn!(
                        "post-dispatch cache lock timeout (3s) for {read_path}, skipping enrichment"
                    );
                }

                // Ledger update — fire-and-forget to avoid blocking concurrent reads.
                // Only real files belong in the context ledger (GL #512): a
                // ctx_read on "." or a directory returns an overview, not file
                // content, and must not appear in the pressure table as a file.
                if std::path::Path::new(&read_path).is_file() {
                    let ledger_clone = self.ledger.clone();
                    let session_clone = self.session.clone();
                    let peer_clone = self.peer.clone();
                    let read_path_owned = read_path.clone();
                    let project_root_owned = project_root.clone();
                    let mode_used =
                        helpers::get_str(args, "mode").unwrap_or_else(|| "auto".to_string());
                    let out_tok = output_tokens as usize;
                    let sent_tok = crate::core::tokens::count_tokens(&result_text);
                    let wants_eviction = true;
                    tokio::spawn(async move {
                        let result = std::panic::AssertUnwindSafe(async {
                            let active_task = {
                                let session = session_clone.read().await;
                                session.task.as_ref().map(|t| t.description.clone())
                            };
                            let mut ledger = ledger_clone.write().await;
                            let overlay = crate::core::context_overlay::OverlayStore::load_project(
                                &std::path::PathBuf::from(
                                    project_root_owned.as_deref().unwrap_or("."),
                                ),
                            );
                            let gate_result = context_gate::post_dispatch_record_with_task(
                                &read_path_owned,
                                &mode_used,
                                out_tok,
                                sent_tok,
                                &mut ledger,
                                &overlay,
                                active_task.as_deref(),
                                project_root_owned.as_deref(),
                            );
                            drop(ledger);
                            if wants_eviction && let Some(hint) = &gate_result.eviction_hint {
                                tracing::debug!("deferred eviction hint: {hint}");
                            }
                            if let Some(hint) = &gate_result.prefetch_hint {
                                tracing::debug!("deferred FEP prefetch hint: {hint}");
                            }
                            if gate_result.resource_changed
                                && let Some(peer) = peer_clone.read().await.as_ref()
                            {
                                notifications::send_resource_updated(
                                    peer,
                                    notifications::RESOURCE_URI_SUMMARY,
                                )
                                .await;
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
                            tracing::error!("background post_dispatch panicked: {msg}");
                        }
                    });
                }
            }
        }

        if !minimal && !is_raw_shell && name == "ctx_shell" {
            let cmd = helpers::get_str(args, "command").unwrap_or_default();

            if let Some(file_path) = extract_file_read_from_shell(&cmd)
                && let Ok(mut bt) = crate::core::bounce_tracker::global().lock()
            {
                bt.next_seq();
                bt.record_shell_file_access(&file_path);
            }
        }

        // Bypass hints are decoupled from minimal_overhead: they ride MCP
        // tool responses (which vary anyway) and don't break provider prompt
        // caching (#498). The `bypass_hints` config key gates them independently.
        if !is_raw_shell && bypass_hint::is_enabled() {
            if let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() {
                let session = self.session.read().await;
                bypass_hint::set_session_id(&session.id);
                drop(session);
                if let Some(hint) = bypass_hint::check(&data_dir) {
                    result_text = format!("{result_text}\n{hint}");
                }
            }
            bypass_hint::record_lctx_call();
        }

        let finding_path_hint = helpers::get_str(args, "path");
        if let Some(finding) = crate::core::auto_findings::extract(
            name,
            &findings_source,
            finding_path_hint.as_deref(),
        ) {
            let mut session = self.session.write().await;
            session.add_finding(finding.file.as_deref(), None, &finding.summary);
            let project_root = session.project_root.clone();
            drop(session);
            if let Some(ref root) = project_root {
                let f = finding.clone();
                let r = root.clone();
                std::thread::spawn(move || {
                    crate::core::auto_capture::capture_finding(&r, &f);
                });
            }
        }
        if let Some(extra) = crate::core::auto_capture::extract_extra(name, &findings_source) {
            let session = self.session.read().await;
            let project_root = session.project_root.clone();
            drop(session);
            if let Some(ref root) = project_root {
                let e = extra.clone();
                let r = root.clone();
                std::thread::spawn(move || {
                    crate::core::auto_capture::capture_finding(&r, &e);
                });
            }
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

        // K-bounded staleness guard: warn if shared context has diverged.
        const K_STALENESS_BOUND: i64 = 10;
        if self.session_mode == crate::tools::SessionMode::Shared
            && let Some(ref rt) = self.context_os
        {
            let latest = rt.bus.latest_id(&self.workspace_id, &self.channel_id);
            let cursor = self
                .last_seen_event_id
                .load(std::sync::atomic::Ordering::Relaxed);
            if cursor > 0 && latest - cursor > K_STALENESS_BOUND {
                let gap = latest - cursor;
                result_text = format!(
                    "[CONTEXT STALE] {gap} events happened since your last read. \
                         Use ctx_session(action=\"status\") to sync.\n\n{result_text}"
                );
            }
            self.last_seen_event_id
                .store(latest, std::sync::atomic::Ordering::Relaxed);
        }

        self.record_receipt_and_cost(
            name,
            args,
            action.as_deref(),
            &result_text,
            output_token_count,
        )
        .await;

        // Context Bus: conflict detection for knowledge writes in shared mode.
        if self.session_mode == crate::tools::SessionMode::Shared
            && name == "ctx_knowledge"
            && action.as_deref() == Some("remember")
            && let Some(ref rt) = self.context_os
        {
            let my_agent = self.agent_id.read().await.clone();
            let category = helpers::get_str(args, "category");
            let key = helpers::get_str(args, "key");
            if let (Some(cat), Some(k)) = (&category, &key) {
                let recent = rt.bus.recent_by_kind(
                    &self.workspace_id,
                    &self.channel_id,
                    "knowledge_remembered",
                    20,
                );
                for ev in &recent {
                    let p = &ev.payload;
                    let ev_cat = p.get("category").and_then(|v| v.as_str());
                    let ev_key = p.get("key").and_then(|v| v.as_str());
                    let ev_actor = ev.actor.as_deref();
                    if ev_cat == Some(cat.as_str())
                        && ev_key == Some(k.as_str())
                        && ev_actor != my_agent.as_deref()
                    {
                        let other = ev_actor.unwrap_or("unknown");
                        result_text = format!(
                            "[CONFLICT] Agent '{other}' recently wrote to the same knowledge key \
                                 '{cat}/{k}'. Review before proceeding.\n\n{result_text}"
                        );
                        break;
                    }
                }
            }
        }

        self.persist_shared_context_os(name, action.as_deref(), args)
            .await;

        let skip_checkpoint =
            minimal || matches!(name, "ctx_session" | "ctx_knowledge" | "ctx_impact");

        // Output-echo nudge (#501): when the agent keeps re-quoting delivered
        // content, tell it once (cooldown-limited, stable text per #498).
        if !skip_checkpoint
            && crate::core::protocol::meta_visible()
            && let Some(nudge) = crate::core::output_echo::take_pending_nudge()
        {
            result_text.push_str(&nudge);
        }

        // #1020: tool-calls.log is now written on the dispatch path
        // (record_call_with_path / record_call_with_timing) with the real
        // original/saved/mode and the measured handler duration. The previous
        // zero-filled append here overwrote every row with `orig=0 saved=0 mode=-`.

        // The only place that advances `call_count`, which the
        // every-100-calls maintenance below reads.
        if !skip_checkpoint {
            self.increment_call_count();
        }

        let current_count = self.call_count.load(std::sync::atomic::Ordering::Relaxed);
        if current_count > 0 && current_count.is_multiple_of(100) {
            // Bound the on-disk archive between restarts: prune TTL-expired and
            // over-budget entries off the hot path so it can't grow unbounded and
            // starve the host of RAM via the page cache (#417).
            std::thread::spawn(|| {
                let _ = crate::core::archive::cleanup();
            });
            // Self-managing memory: opportunistically consolidate knowledge in the
            // background (time-gated + single-flight inside `maybe_run`).
            if let Some(root) = self.session.read().await.project_root.clone() {
                crate::core::cognition_scheduler::maybe_run(&root);
            }
        }

        // #990: a machine-readable invocation (e.g. ctx_outline format=json) must
        // return a byte-exact, parseable payload. The state-consuming briefings
        // are already skipped above (so their once-per-session flags survive),
        // but other steps still append recomputed prose (verify footer, throttle
        // / budget warning, deprecation notice) or compress the body — all of
        // which break a JSON contract. This guard is the robust catch-all:
        // restore the pure body captured *before* compression and decoration.
        // Redaction + sensitivity were applied earlier so the security envelope
        // is preserved. `ctx_outline` is not an archivable/firewallable tool, so
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
/// (GitHub #389): a failing or blocked command sets `isError: true` and a
/// `structuredContent` payload (`{"exitCode": N}` / `{"blocked": true}`), so
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
        crate::server::tool_trait::ShellOutcome::Exit(_)
        | crate::server::tool_trait::ShellOutcome::Blocked => true,
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
    fn blocked_command_sets_is_error_and_blocked_marker() {
        let r = finalize_call_result("[BLOCKED] nope", Some(ShellOutcome::Blocked));
        assert_eq!(
            r.is_error,
            Some(true),
            "blocked commands never ran — that is a failure"
        );
        assert_eq!(
            r.structured_content,
            Some(serde_json::json!({ "blocked": true }))
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
