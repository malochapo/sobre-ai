//! Post-dispatch side-effect stages for `SobreServer::call_tool_guarded`
//! (issue #144).
//!
//! These blocks run *after* a tool produced its (already post-processed) output
//! and perform pure side effects — they never mutate the text returned to the
//! model. They are tightly coupled to `&self` (locks, peers, spawned tasks), so
//! they live as thin `SobreServer` methods rather than free functions. Moving
//! them out of the guarded path keeps that function a readable orchestrator;
//! behaviour, ordering and await points are identical to the inlined versions.

#[allow(clippy::wildcard_imports)]
use super::*;

impl SobreServer {
    /// Record the tool receipt, infer/apply intent, persist the session when due,
    /// and trigger auto-consolidation. All work is either
    /// synchronous bookkeeping under the session lock or fire-and-forget blocking
    /// tasks; nothing here feeds back into the tool output.
    pub(super) async fn record_receipt(
        &self,
        name: &str,
        args: Option<&serde_json::Map<String, serde_json::Value>>,
        action: Option<&str>,
        result_text: &str,
    ) {
        let input = helpers::canonical_args_string(args);
        let input_md5 = helpers::hash_fast(&input);
        let output_md5 = helpers::hash_fast(result_text);
        let agent_id = self.agent_id.read().await.clone();
        let client_name = self.client_name.read().await.clone();

        let pending_session_save = {
            let mut session = self.session.write().await;
            session.record_tool_receipt(
                name,
                action,
                &input_md5,
                &output_md5,
                agent_id.as_deref(),
                Some(&client_name),
            );

            if session.should_save() {
                session.prepare_save().ok()
            } else {
                None
            }
        };

        if let Some(prepared) = pending_session_save {
            tokio::task::spawn_blocking(move || {
                let _ = prepared.write_to_disk();
                // Flush the persistent stub index on the same batch cadence so
                // recent full deliveries survive a restart as cheap stubs (#955).
                crate::core::read_stub_index::persist();
            });
        }
    }
}
