use std::sync::atomic::Ordering;

use super::server::SobreServer;

impl SobreServer {
    /// Per-call bookkeeping that is not usage: the session's own counters. Usage goes to the ledger from `call_tool`, once
    /// the final sent-token count is known.
    pub async fn record_call(&self, tool: &str, original: usize, saved: usize) {
        let mut session = self.session.write().await;
        session.record_tool_call(saved as u64, original as u64);
        if tool == "sobre_shell" {
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
