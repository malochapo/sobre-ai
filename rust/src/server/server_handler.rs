//! `rmcp::ServerHandler` trait implementation for [`SobreServer`].
//!
//! Split out of `server/mod.rs`; `use super::*` re-imports the parent module’s
//! aliases and sibling submodules. Methods attach to `SobreServer` regardless
//! of which module the impl block lives in.

#[allow(clippy::wildcard_imports)]
use super::*;

/// The advertised MCP server capabilities: tools only. The tool set never
/// changes during a session (invariant 3), so `listChanged` is not declared.
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities::builder().enable_tools().build()
}

impl ServerHandler for SobreServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = server_capabilities();

        let instructions = crate::instructions::build_instructions();

        InitializeResult::new(capabilities)
            .with_server_info(Implementation::new("sobre", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        let name = request.client_info.name.clone();
        tracing::info!("MCP client connected: {:?}", name);
        *self.client_name.write().await = name.clone();
        *self.peer.write().await = Some(context.peer.clone());

        let has_roots = request.capabilities.roots.is_some();
        self.has_client_roots
            .store(has_roots, std::sync::atomic::Ordering::Relaxed);
        if has_roots {
            tracing::info!("Client supports MCP roots/list — will resolve on first tool call");
        }

        let env_root = roots::root_from_env().or_else(roots::root_from_workspace_env);
        let derived_root = derive_project_root_from_cwd();
        let effective_root = env_root.or(derived_root);

        let cwd_str = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        {
            let mut session = self.session.write().await;
            if !cwd_str.is_empty() {
                session.shell_cwd = Some(cwd_str.clone());
            }
            if let Some(ref root) = effective_root {
                session.project_root = Some(root.clone());
                tracing::info!("Project root set to: {root}");
                // Cursor multi-root: register sibling workspace folders as extra
                // roots.
                for other in roots::workspace_roots_from_env() {
                    if &other != root && !session.extra_roots.contains(&other) {
                        session.extra_roots.push(other);
                    }
                }
            } else {
                // No host signal: never adopt the previous session's root,
                // which may belong to another project. MCP roots/list or the
                // cwd decide on the first tool call.
                session.project_root = None;
            }
            // Search roots are the extra roots of a multi-root
            // workspace, except a broad root ("/", $HOME).
            let cfg_extra: Vec<String> = crate::core::config::Config::load()
                .search_roots
                .into_iter()
                .filter(|p| {
                    !crate::core::pathutil::is_broad_or_unsafe_root(std::path::Path::new(p.trim()))
                })
                .collect();
            if !cfg_extra.is_empty() {
                let existing: std::collections::HashSet<_> =
                    session.extra_roots.iter().cloned().collect();
                for r in cfg_extra {
                    if !existing.contains(&r) {
                        session.extra_roots.push(r);
                    }
                }
            }
            if let Err(e) = session.save() {
                tracing::warn!("sobre: failed to persist session state: {e}");
            }
        }

        // Indices are warmed lazily on first use of a tool that needs them
        //, not eagerly here — a session that only uses
        // sobre_read/sobre_shell/sobre_tree must not pay a full graph + BM25 scan.
        // See `index_orchestrator::ensure_warm_for_tool`, driven from dispatch.

        let agent_root = effective_root.clone().unwrap_or_default();
        let agent_id_handle = self.agent_id.clone();
        let presence_id = self.presence_agent_id.read().await.clone();
        tokio::task::spawn_blocking(move || {
            // Avoid startup stampedes when multiple agent sessions initialize at once.
            // These are best-effort maintenance tasks; it's fine to skip if another
            // sobre instance is already doing them.
            let maintenance = crate::core::startup_guard::try_acquire_lock(
                "startup-maintenance",
                std::time::Duration::from_secs(2),
                std::time::Duration::from_mins(2),
            );
            if maintenance.is_some() {
                // Prune accumulated quarantined BM25 indexes. Silent
                // (tracing only) so it never corrupts the MCP stdio protocol.
                let _ = crate::core::storage_maintenance::run_quiet();
            }
            drop(maintenance);

            // The process registered itself at startup; the session reuses that
            // entry, pointed at the root it settled on, instead of adding a second.
            if !agent_root.is_empty() {
                let id = crate::core::agents::AgentRegistry::mutate_locked(|registry| {
                    if let Some(id) = presence_id {
                        registry.set_project_root(&id, &agent_root);
                        id
                    } else {
                        registry.cleanup_stale(24);
                        registry.register("mcp", Some("context-engine"), &agent_root)
                    }
                })
                .map(|(_, id)| id)
                .ok();
                if let (Some(id), Ok(mut guard)) = (id, agent_id_handle.try_write()) {
                    *guard = Some(id);
                }
            }
        });

        let instructions = crate::instructions::build_instructions();

        let capabilities = server_capabilities();

        Ok(InitializeResult::new(capabilities)
            .with_server_info(Implementation::new("sobre", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // Panic guard (mirrors call_tool): a panic here must not kill the rmcp
        // request task.
        use std::panic::AssertUnwindSafe;
        let computed = AssertUnwindSafe(async {
            // Invariant 3: exactly one answer. The registry is the surface; only
            // the descriptions are compressed.
            let tools = crate::server::tool_visibility::advertised(self.registry.tool_defs());

            Ok(ListToolsResult {
                tools,
                ..Default::default()
            })
        })
        .catch_unwind()
        .await;
        computed.unwrap_or_else(|_| {
            // A panic here must NOT leave the agent tool-less — that is
            // indistinguishable from "MCP totally failed" and gives the user no
            // recovery path. Fall back to the registry's uncompressed defs.
            tracing::error!("list_tools panicked; serving uncompressed tool descriptions");
            Ok(ListToolsResult {
                tools: self.registry.tool_defs(),
                ..Default::default()
            })
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        use std::panic::AssertUnwindSafe;

        // Progress belongs to this request: its token, its peer.
        let progress = request
            .meta
            .as_ref()
            .and_then(rmcp::model::Meta::get_progress_token)
            .map(|token| crate::server::progress::ProgressSender::new(context.peer.clone(), token));

        match AssertUnwindSafe(self.call_tool_guarded(request, progress))
            .catch_unwind()
            .await
        {
            Ok(result) => result,
            Err(panic_payload) => {
                let detail = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown".to_string()
                };
                tracing::error!("call_tool panicked: {detail}");

                Ok(CallToolResult::error(vec![ContentBlock::text(
                    "ERROR: sobre internal error. The MCP server is still running. \
                     Please retry or use a different approach."
                        .to_string(),
                )]))
            }
        }
    }

    async fn on_roots_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleServer>,
    ) {
        tracing::info!("Received roots/list_changed — will re-resolve on next tool call");
        self.roots_resolved
            .store(false, std::sync::atomic::Ordering::Relaxed);
        // Fresh client signal — restore the transient-failure retry budget.
        self.roots_list_attempts
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_capabilities_never_declare_tool_list_changed() {
        let caps = server_capabilities();
        let tools = caps.tools.expect("tools capability must be advertised");
        assert_ne!(tools.list_changed, Some(true));
        assert!(caps.resources.is_none() && caps.prompts.is_none());
    }

    /// The `list_tools` panic guard serves the registry, so an internal panic
    /// never leaves the agent tool-less — indistinguishable from "MCP totally
    /// failed". Regression guard.
    ///
    /// It used to serve a hand-maintained static list, which is how the
    /// advertised surface drifted from the registry in the first place.
    #[test]
    fn panic_fallback_serves_the_registry() {
        let _guard = crate::core::data_dir::isolated_data_dir();
        let defs = crate::server::registry::build_registry().tool_defs();
        assert!(!defs.is_empty(), "the fallback must not be empty");
        for essential in ["sobre_read", "sobre_shell"] {
            assert!(
                defs.iter().any(|t| t.name.as_ref() == essential),
                "{essential} must survive the fallback"
            );
        }
    }
}
