//! `rmcp::ServerHandler` trait implementation for [`SobreServer`].
//!
//! Split out of `server/mod.rs`; `use super::*` re-imports the parent module’s
//! aliases and sibling submodules. Methods attach to `SobreServer` regardless
//! of which module the impl block lives in.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Builds the advertised MCP server capabilities. The tool set never changes
/// during a session (invariant 3), so `listChanged` is not declared. The
/// `resources`/`prompts` surfaces stay client-gated so we never advertise a
/// surface the connected client cannot use.
fn server_capabilities(resources: bool, prompts: bool) -> ServerCapabilities {
    match (resources, prompts) {
        (true, true) => ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_resources_subscribe()
            .enable_prompts()
            .build(),
        (true, false) => ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_resources_subscribe()
            .build(),
        (false, true) => ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .build(),
        (false, false) => ServerCapabilities::builder().enable_tools().build(),
    }
}

impl ServerHandler for SobreServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = server_capabilities(true, true);

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
                // roots (#699).
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
        // (issue #152), not eagerly here — a session that only uses
        // ctx_read/ctx_shell/ctx_tree must not pay a full graph + BM25 scan.
        // See `index_orchestrator::ensure_warm_for_tool`, driven from dispatch.

        let agent_root = effective_root.clone().unwrap_or_default();
        let agent_id_handle = self.agent_id.clone();
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
                // Prune accumulated quarantined BM25 indexes (#2364). Silent
                // (tracing only) so it never corrupts the MCP stdio protocol.
                let _ = crate::core::storage_maintenance::run_quiet();
            }
            drop(maintenance);

            if !agent_root.is_empty() {
                let id = crate::core::agents::AgentRegistry::mutate_locked(|registry| {
                    registry.cleanup_stale(24);
                    registry.register("mcp", None, &agent_root)
                })
                .map(|(_, id)| id)
                .ok();
                if let (Some(id), Ok(mut guard)) = (id, agent_id_handle.try_write()) {
                    *guard = Some(id);
                }
            }
        });

        let client_caps = crate::core::client_capabilities::ClientMcpCapabilities::detect(&name);
        tracing::info!("Client capabilities: {}", client_caps.format_summary());

        crate::core::client_capabilities::set_detected(&client_caps);

        let instructions = crate::instructions::build_instructions();

        let capabilities = server_capabilities(client_caps.resources, client_caps.prompts);

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
            // Invariant 3: exactly one answer. The registry IS the surface —
            // no candidate set, no client quirks, no profile, no role, no
            // workflow state, no lazy core.
            //
            // What used to be here filtered the registry through nine axes. One
            // of them silently broke the product: `CORE_TOOL_NAMES` still named
            // `shell` and `ctx_call` after both left the registry, so the
            // default path (no config, no env, no pinned profile) advertised
            // their intersection — ten tools out of nineteen, with no error.
            let tools = if let Some(ref reg) = self.registry {
                reg.tool_defs()
            } else {
                // Every production constructor sets a registry; locked by
                // `production_server_always_has_registry`.
                tracing::error!("list_tools served without a tool registry");
                Vec::new()
            };

            // Description compression is not a surface variation: the set of
            // tools is identical, only the prose is squeezed. That squeeze is
            // what the product is for.
            let tools = {
                let cfg = crate::core::config::Config::load();
                let level = crate::core::config::CompressionLevel::effective(&cfg);
                let mode =
                    crate::core::terse::mcp_compress::DescriptionMode::from_compression_level(
                        &level,
                    );
                if mode == crate::core::terse::mcp_compress::DescriptionMode::Full {
                    tools
                } else {
                    tools
                        .into_iter()
                        .map(|mut t| {
                            let compressed = crate::core::terse::mcp_compress::compress_description(
                                t.name.as_ref(),
                                t.description.as_deref().unwrap_or(""),
                                mode,
                            );
                            t.description = Some(compressed.into());
                            t
                        })
                        .collect()
                }
            };

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
            // recovery path. Fall back to the static lazy-core defs (a pure,
            // panic-free function) so ctx_read/ctx_shell stay available
            // even if the dynamic/registry path blew up.
            tracing::error!(
                "list_tools panicked; serving the static lazy-core tool set as a fallback"
            );
            Ok(ListToolsResult {
                tools: self
                    .registry
                    .as_ref()
                    .map(|r| r.tool_defs())
                    .unwrap_or_default(),
                ..Default::default()
            })
        })
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListPromptsResult, ErrorData>> {
        std::future::ready(Ok(rmcp::model::ListPromptsResult::with_all_items(
            prompts::list_prompts(),
        )))
    }

    async fn get_prompt(
        &self,
        request: rmcp::model::GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::GetPromptResult, ErrorData> {
        let ledger = self.ledger.read().await;
        match prompts::get_prompt(&request, &ledger) {
            Some(result) => Ok(result),
            None => Err(ErrorData::invalid_params(
                format!("Unknown prompt: {}", request.name),
                None,
            )),
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListResourcesResult, rmcp::ErrorData>> {
        std::future::ready(Ok(rmcp::model::ListResourcesResult::with_all_items(
            resources::list_resources(),
        )))
    }

    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResult, rmcp::ErrorData> {
        let ledger = self.ledger.read().await;
        match resources::read_resource(&request.uri, &ledger) {
            Some(contents) => Ok(rmcp::model::ReadResourceResult::new(contents)),
            None => Err(rmcp::ErrorData::resource_not_found(
                format!("Unknown resource: {}", request.uri),
                None,
            )),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        use std::panic::AssertUnwindSafe;

        let progress_token = request
            .meta
            .as_ref()
            .and_then(rmcp::model::Meta::get_progress_token);
        if let Some(ref token) = progress_token {
            let sender =
                crate::server::progress::ProgressSender::new(context.peer.clone(), token.clone());
            *self
                .progress_sender
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
        }

        match AssertUnwindSafe(self.call_tool_guarded(request))
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
        for (resources, prompts) in [(true, true), (true, false), (false, true), (false, false)] {
            let caps = server_capabilities(resources, prompts);
            let tools = caps.tools.expect("tools capability must be advertised");
            assert_ne!(tools.list_changed, Some(true));
        }
    }

    /// The `list_tools` panic guard serves the registry, so an internal panic
    /// never leaves the agent tool-less — indistinguishable from "MCP totally
    /// failed". Regression guard for #688.
    ///
    /// It used to serve a hand-maintained static list, which is how the
    /// advertised surface drifted from the registry in the first place.
    #[test]
    fn panic_fallback_serves_the_registry() {
        let _guard = crate::core::data_dir::isolated_data_dir();
        let defs = crate::server::registry::build_registry().tool_defs();
        assert!(!defs.is_empty(), "the fallback must not be empty");
        for essential in ["ctx_read", "ctx_shell"] {
            assert!(
                defs.iter().any(|t| t.name.as_ref() == essential),
                "{essential} must survive the fallback"
            );
        }
    }
}
