//! Post-dispatch side-effect stages for `LeanCtxServer::call_tool_guarded`
//! (issue #144).
//!
//! These blocks run *after* a tool produced its (already post-processed) output
//! and perform pure side effects — they never mutate the text returned to the
//! model. They are tightly coupled to `&self` (locks, peers, spawned tasks), so
//! they live as thin `LeanCtxServer` methods rather than free functions. Moving
//! them out of the guarded path keeps that function a readable orchestrator;
//! behaviour, ordering and await points are identical to the inlined versions.

#[allow(clippy::wildcard_imports)]
use super::*;

impl LeanCtxServer {
    /// Record the tool receipt, infer/apply intent, persist the session when due,
    /// trigger auto-consolidation, and attribute token cost. All work is either
    /// synchronous bookkeeping under the session lock or fire-and-forget blocking
    /// tasks; nothing here feeds back into the tool output.
    pub(super) async fn record_receipt_and_cost(
        &self,
        name: &str,
        args: Option<&serde_json::Map<String, serde_json::Value>>,
        action: Option<&str>,
        result_text: &str,
        output_token_count: usize,
    ) {
        let input = helpers::canonical_args_string(args);
        let input_md5 = helpers::hash_fast(&input);
        let output_md5 = helpers::hash_fast(result_text);
        let agent_id = self.agent_id.read().await.clone();
        let client_name = self.client_name.read().await.clone();
        let mut explicit_intent: Option<(
            crate::core::intent_protocol::IntentRecord,
            Option<String>,
            String,
        )> = None;

        let pending_session_save = {
            let empty_args = serde_json::Map::new();
            let args_map = args.unwrap_or(&empty_args);
            let mut session = self.session.write().await;
            session.record_tool_receipt(
                name,
                action,
                &input_md5,
                &output_md5,
                agent_id.as_deref(),
                Some(&client_name),
            );

            if let Some(intent) = crate::core::intent_protocol::infer_from_tool_call(
                name,
                action,
                args_map,
                session.project_root.as_deref(),
            ) {
                let is_explicit =
                    intent.source == crate::core::intent_protocol::IntentSource::Explicit;
                let root = session.project_root.clone();
                let sid = session.id.clone();
                session.record_intent(intent.clone());
                if is_explicit {
                    explicit_intent = Some((intent, root, sid));
                }
            }
            if session.should_save() {
                session.prepare_save().ok()
            } else {
                None
            }
        };

        if let Some(prepared) = pending_session_save {
            let ir_clone = self.context_ir.clone();
            tokio::task::spawn_blocking(move || {
                let _ = prepared.write_to_disk();
                if let Some(ir) = ir_clone
                    && let Ok(ir_guard) = ir.try_read()
                {
                    ir_guard.save();
                }
                // Flush the persistent stub index on the same batch cadence so
                // recent full deliveries survive a restart as cheap stubs (#955).
                crate::core::read_stub_index::persist();
            });
        }

        if let Some((intent, root, session_id)) = explicit_intent {
            let _ = crate::core::intent_protocol::apply_side_effects(
                &intent,
                root.as_deref(),
                &session_id,
            );
        }

        {
            let (calls, project_root) = {
                let session = self.session.read().await;
                (session.stats.total_tool_calls, session.project_root.clone())
            };

            if let Some(root) = project_root
                && crate::tools::autonomy::should_auto_consolidate(&self.autonomy, calls)
            {
                let root_clone = root.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = crate::core::consolidation_engine::consolidate_latest(
                        &root_clone,
                        crate::core::consolidation_engine::ConsolidationBudgets::default(),
                    );
                });
            }
        }

        let agent_key = agent_id.unwrap_or_else(|| "unknown".to_string());
        let input_token_count = crate::core::tokens::count_tokens(&input) as u64;
        let output_token_count_u64 = output_token_count as u64;
        let name_owned = name.to_string();
        tokio::task::spawn_blocking(move || {
            let mut store = crate::core::a2a::cost_attribution::CostStore::load();
            store.record_tool_call(
                &agent_key,
                &client_name,
                &name_owned.clone(),
                input_token_count,
                output_token_count_u64,
                0,
            );
            if let Err(e) = store.save() {
                tracing::warn!("lean-ctx: failed to persist cost attribution: {e}");
            }
        });
    }
}
