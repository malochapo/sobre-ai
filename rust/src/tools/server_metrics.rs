use std::sync::atomic::Ordering;

use super::server::{CepComputedStats, LeanCtxServer, ToolCallRecord};

impl LeanCtxServer {
    /// Records a tool call's token savings without timing information.
    pub async fn record_call(
        &self,
        tool: &str,
        original: usize,
        saved: usize,
        mode: Option<String>,
    ) {
        self.record_call_with_timing(tool, original, saved, mode, 0)
            .await;
    }

    /// Records a tool call like `record_call`, but includes an optional file
    /// path for observability and the measured handler duration (#1020 — the
    /// duration is what makes the row land in `tool-calls.log`).
    pub async fn record_call_with_path(
        &self,
        tool: &str,
        original: usize,
        saved: usize,
        mode: Option<String>,
        path: Option<&str>,
        duration_ms: u64,
    ) {
        self.record_call_with_timing_inner(tool, original, saved, mode, duration_ms, path)
            .await;
    }

    /// Records a tool call's token savings, duration, and emits events and stats.
    pub async fn record_call_with_timing(
        &self,
        tool: &str,
        original: usize,
        saved: usize,
        mode: Option<String>,
        duration_ms: u64,
    ) {
        self.record_call_with_timing_inner(tool, original, saved, mode, duration_ms, None)
            .await;
    }

    async fn record_call_with_timing_inner(
        &self,
        tool: &str,
        original: usize,
        saved: usize,
        mode: Option<String>,
        duration_ms: u64,
        path: Option<&str>,
    ) {
        if let Some(agent_id) = self.presence_agent_id.read().await.clone()
            && let Err(error) = crate::core::agents::AgentRegistry::heartbeat_persistent(&agent_id)
        {
            tracing::warn!("lean-ctx: failed to update MCP agent heartbeat: {error}");
        }
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut calls = self.tool_calls.write().await;
        calls.push(ToolCallRecord {
            tool: tool.to_string(),
            original_tokens: original,
            saved_tokens: saved,
            mode: mode.clone(),
            duration_ms,
            timestamp: ts.clone(),
        });

        const MAX_TOOL_CALL_RECORDS: usize = 500;
        if calls.len() > MAX_TOOL_CALL_RECORDS {
            let excess = calls.len() - MAX_TOOL_CALL_RECORDS;
            calls.drain(..excess);
        }

        // #1020: persist whenever we have a duration OR real metrics. The old
        // `duration_ms > 0` gate dropped every read/search row (they record with
        // measured metrics but were previously logged via a separate zero-filled
        // path), so `tool-calls.log` only ever showed `orig=0 saved=0 mode=-`.
        if duration_ms > 0 || original > 0 {
            Self::append_tool_call_log(tool, duration_ms, original, saved, mode.as_deref(), &ts);
        }

        crate::core::events::emit_tool_call(
            tool,
            original as u64,
            saved as u64,
            mode.clone(),
            duration_ms,
            path.map(ToString::to_string),
        );

        let output_tokens = original.saturating_sub(saved);
        crate::core::stats::record_at_turn(
            tool,
            original,
            output_tokens,
            crate::core::context_overhead::observed_turns(),
        );
        // MCP shell savings are measured (raw vs compressed output), so they are
        // ledger-grade (GL #479 D2). Reads are ledgered by the ctx_read /
        // ctx_multi_read callers (#685, decoupled from the heatmap) and ctx_search
        // records itself — only shell is recorded here, exactly once. `actual_tokens`
        // is the *sent* output; a prior duplicate block passed `saved` and so both
        // double-counted shell events and stored the wrong saving (#685).
        if tool == "ctx_shell" {
            crate::core::savings_ledger::record_tool_event(tool, original, output_tokens);
        }

        // Heatmap: every path-bearing tool call feeds the per-file pressure
        // view. Note the double count: `ctx_read`
        // and `ctx_multi_read` also record the heatmap from the read path, so
        // a read lands here twice. Tools that carry a path but do not record
        // themselves (`ctx_patch`) land here only. Deciding what the counts
        // should be is a separate change.
        if let Some(file_path) = path
            && !file_path.is_empty()
            && original > 0
        {
            let agent_id = self
                .presence_agent_id
                .read()
                .await
                .clone()
                .unwrap_or_default();
            crate::core::heatmap::record_file_access_with_agent(
                file_path,
                original,
                saved.min(original),
                Some(&agent_id),
            );
        }
        let mut session = self.session.write().await;
        session.record_tool_call(saved as u64, original as u64);
        if tool == "ctx_shell" {
            session.record_command();
        }
        let pending_save = if session.should_save() {
            session.prepare_save().ok()
        } else {
            None
        };
        drop(calls);
        drop(session);

        if let Some(prepared) = pending_save {
            tokio::task::spawn_blocking(move || {
                let _ = prepared.write_to_disk();
            });
        }

        self.write_mcp_live_stats().await;
    }

    /// Increments the tool-call counter.
    pub fn increment_call_count(&self) {
        self.call_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Appends a tool call entry to the rotating `tool-calls.log` file.
    pub fn append_tool_call_log(
        tool: &str,
        duration_ms: u64,
        original: usize,
        saved: usize,
        mode: Option<&str>,
        timestamp: &str,
    ) {
        const MAX_LOG_LINES: usize = 50;
        if let Ok(dir) = crate::core::paths::state_dir() {
            let log_path = dir.join("tool-calls.log");
            let mode_str = mode.unwrap_or("-");
            let slow = if duration_ms > 5000 { " **SLOW**" } else { "" };
            let line = format!(
                "{timestamp}\t{tool}\t{duration_ms}ms\torig={original}\tsaved={saved}\tmode={mode_str}{slow}\n"
            );

            let mut lines: Vec<String> = std::fs::read_to_string(&log_path)
                .unwrap_or_default()
                .lines()
                .map(std::string::ToString::to_string)
                .collect();

            lines.push(line.trim_end().to_string());
            if lines.len() > MAX_LOG_LINES {
                lines.drain(0..lines.len() - MAX_LOG_LINES);
            }

            let _ = std::fs::write(&log_path, lines.join("\n") + "\n");
        }
    }

    fn compute_cep_stats(
        calls: &[ToolCallRecord],
        stats: &crate::core::cache::CacheStats,
        complexity: &crate::core::adaptive::TaskComplexity,
    ) -> CepComputedStats {
        let total_original: u64 = calls.iter().map(|c| c.original_tokens as u64).sum();
        let total_saved: u64 = calls.iter().map(|c| c.saved_tokens as u64).sum();
        let total_compressed = total_original.saturating_sub(total_saved);
        let compression_rate = if total_original > 0 {
            total_saved as f64 / total_original as f64
        } else {
            0.0
        };

        let modes_used: std::collections::HashSet<&str> =
            calls.iter().filter_map(|c| c.mode.as_deref()).collect();
        let mode_diversity = (modes_used.len() as f64 / 10.0).min(1.0);
        let cache_util = stats.hit_rate() / 100.0;
        // Output efficiency (#501): 1 - avg echo ratio. An agent that keeps
        // re-quoting delivered content burns the input savings on output.
        let output_efficiency = 1.0 - crate::core::output_echo::current_avg_ratio();
        let cep_score = cache_util * 0.25
            + mode_diversity * 0.15
            + compression_rate * 0.45
            + output_efficiency * 0.15;

        let mut mode_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for call in calls {
            if let Some(ref mode) = call.mode {
                *mode_counts.entry(mode.clone()).or_insert(0) += 1;
            }
        }

        CepComputedStats {
            cep_score: (cep_score * 100.0).round() as u32,
            cache_util: (cache_util * 100.0).round() as u32,
            mode_diversity: (mode_diversity * 100.0).round() as u32,
            compression_rate: (compression_rate * 100.0).round() as u32,
            total_original,
            total_compressed,
            total_saved,
            mode_counts,
            complexity: format!("{complexity:?}"),
            cache_hits: stats.cache_hits(),
            total_reads: stats.total_reads(),
            tool_call_count: calls.len() as u64,
        }
    }

    async fn write_mcp_live_stats(&self) {
        let count = self.call_count.load(Ordering::Relaxed);
        if count > 1 && !count.is_multiple_of(5) {
            return;
        }

        let cache = self.cache.read().await;
        let calls = self.tool_calls.read().await;
        let stats = cache.get_stats();
        let complexity = crate::core::adaptive::classify_from_context(&cache);

        let cs = Self::compute_cep_stats(&calls, stats, &complexity);
        let started_at = calls
            .first()
            .map(|c| c.timestamp.clone())
            .unwrap_or_default();

        drop(cache);
        drop(calls);

        // Persist CEP on the live-stats cadence (first call + every 5th) so even
        // short sessions register `sessions`/`total_cache_hits` instead of only
        // recording on an `auto_checkpoint` that a brief workload may never reach.
        // `record_cep_session` is delta-based and PID-guarded, so the extra call
        // that coincides with a checkpoint is a no-op for the totals (#361).
        crate::core::stats::record_cep_session(
            cs.cep_score,
            cs.cache_hits,
            cs.total_reads,
            cs.total_original,
            cs.total_compressed,
            &cs.mode_counts,
            cs.tool_call_count,
            &cs.complexity,
        );

        let live = serde_json::json!({
            "cep_score": cs.cep_score,
            "cache_utilization": cs.cache_util,
            "mode_diversity": cs.mode_diversity,
            "compression_rate": cs.compression_rate,
            "task_complexity": cs.complexity,
            "files_cached": cs.total_reads,
            "total_reads": cs.total_reads,
            "cache_hits": cs.cache_hits,
            "tokens_saved": cs.total_saved,
            "tokens_original": cs.total_original,
            "tool_calls": cs.tool_call_count,
            "started_at": started_at,
            "updated_at": chrono::Local::now().to_rfc3339(),
        });

        if let Ok(dir) = crate::core::paths::state_dir() {
            let _ = std::fs::write(dir.join("mcp-live.json"), live.to_string());
        }
    }
}
