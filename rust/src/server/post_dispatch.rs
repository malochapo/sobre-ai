//! Post-dispatch side-effect stages for `SobreServer::call_tool_guarded`.
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
    /// Count the call as a session change and persist the session when due.
    /// Fire-and-forget: nothing here feeds back into the tool output.
    pub(super) async fn save_session_when_due(&self) {
        let pending_session_save = {
            let mut session = self.session.write().await;
            session.increment();

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
                // recent full deliveries survive a restart as cheap stubs.
                crate::core::read_stub_index::persist();
            });
        }
    }
}
