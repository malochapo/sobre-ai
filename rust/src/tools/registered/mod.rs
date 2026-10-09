pub(crate) mod batch_read;
pub mod sobre_checkpoint;
pub mod sobre_compose;
pub mod sobre_expand;
pub mod sobre_glob;
pub mod sobre_graph;
pub mod sobre_knowledge;
pub mod sobre_outline;
pub mod sobre_patch;
pub mod sobre_read;
pub mod sobre_refactor;
pub mod sobre_review;
pub mod sobre_search;
pub mod sobre_session;
pub mod sobre_shell;
pub mod sobre_tree;

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

// Tool descriptions do not steer toward sobre_* over native tools; that guidance
// lives in one place, the MCP server instructions (`crate::instructions`).
