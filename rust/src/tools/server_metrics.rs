use std::sync::atomic::Ordering;

use super::server::SobreServer;

impl SobreServer {
    /// Per-call bookkeeping that is not usage: the agent heartbeat and the
    /// session's own counters. Usage goes to the ledger from `call_tool`, once
    /// the final sent-token count is known.
    pub async fn record_call(&self, tool: &str, original: usize, saved: usize) {
        if let Some(agent_id) = self.presence_agent_id.read().await.clone()
            && let Err(error) = crate::core::agents::AgentRegistry::heartbeat_persistent(&agent_id)
        {
            tracing::warn!("sobre: failed to update MCP agent heartbeat: {error}");
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
        drop(session);

        if let Some(prepared) = pending_save {
            tokio::task::spawn_blocking(move || {
                let _ = prepared.write_to_disk();
            });
        }
    }

    /// Increments the tool-call counter.
    pub fn increment_call_count(&self) {
        self.call_count.fetch_add(1, Ordering::Relaxed);
    }
}
