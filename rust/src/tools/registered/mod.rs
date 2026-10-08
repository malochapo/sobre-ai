pub(crate) mod batch_read;
pub mod ctx_checkpoint;
pub mod ctx_compose;
pub mod ctx_expand;
pub mod ctx_glob;
pub mod ctx_graph;
pub mod ctx_knowledge;
pub mod ctx_outline;
pub mod ctx_patch;
pub mod ctx_read;
pub mod ctx_refactor;
pub mod ctx_review;
pub mod ctx_search;
pub mod ctx_session;
pub mod ctx_shell;
pub mod ctx_tree;
pub(crate) mod impl_edit;

#[cfg(test)]
mod schema_qa_tests;

/// Resolve a relative path against session state (sync version).
/// Must be called within `tokio::task::block_in_place`.
pub(crate) fn resolve_path_sync(
    session: &crate::core::session::SessionState,
    raw: &str,
) -> Result<String, String> {
    crate::core::path_resolve::resolve_tool_path(
        session.project_root.as_deref(),
        session.shell_cwd.as_deref(),
        raw,
    )
}

// Tool descriptions do not steer toward ctx_* over native tools; that guidance
// lives in one place, the MCP server instructions (`crate::instructions`).
