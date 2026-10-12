//! Post-dispatch side effects for `SobreServer::call_tool_guarded`: work that
//! runs *after* a tool produced its (already post-processed) output and never
//! changes the text returned to the model.

/// Write the persistent stub index once its changes are due, off the async
/// workers, so recent full deliveries survive a restart as cheap stubs.
/// Fire-and-forget: nothing here feeds back into the tool output.
pub(super) fn persist_stub_index_when_due() {
    if crate::core::read_stub_index::flush_due() {
        tokio::task::spawn_blocking(crate::core::read_stub_index::persist);
    }
}
