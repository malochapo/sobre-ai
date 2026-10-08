use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::core::cache::SessionCache;
use crate::core::session::SessionState;

use super::autonomy;
use super::server::SobreServer;
use super::startup::detect_startup_context;

impl Default for SobreServer {
    fn default() -> Self {
        Self::new()
    }
}

impl SobreServer {
    /// Creates a new server with default settings, auto-detecting the project root.
    pub fn new() -> Self {
        Self::new_with_project_root(None)
    }

    /// Creates a new server rooted at the given project directory.
    pub fn new_with_project_root(project_root: Option<&str>) -> Self {
        Self::new_with_startup(project_root, std::env::current_dir().ok().as_deref())
    }

    pub(crate) fn new_with_startup(project_root: Option<&str>, startup_cwd: Option<&Path>) -> Self {
        let ttl = crate::core::config::IDLE_CACHE_TTL_SECS;

        // Purge stale graph indices on startup to prevent serving outdated data
        crate::core::graph_index::ProjectIndex::purge_stale_indices();

        let startup = detect_startup_context(project_root, startup_cwd);
        let mut session = if let Some(ref root) = startup.project_root {
            SessionState::load_latest_for_project_root(root).unwrap_or_default()
        } else {
            SessionState::load_latest().unwrap_or_default()
        };
        if let Some(ref root) = startup.project_root {
            session.project_root = Some(root.clone());
        }
        if let Some(ref cwd) = startup.shell_cwd {
            session.shell_cwd = Some(cwd.clone());
        }
        let session = Arc::new(RwLock::new(session));

        // Indices are NOT built eagerly here. A freshly connected agent that sits
        // idle — or only uses sobre_read/sobre_shell/sobre_tree — must pay zero indexing
        // cost. Heavy/search tools warm their indices lazily on first use via
        // `index_orchestrator::ensure_warm_for_tool`, driven from dispatch (#152).
        // An eager full graph + BM25 scan on every `new()` pegged a CPU core on
        // each server start; multiplied across multiple agents and stdio respawns
        // it was the root cause of the idle-high-CPU report (#453).

        // Rehydrate the persistent stub index (#955) so the first unchanged
        // re-read after this restart can collapse to the `[unchanged]` stub
        // instead of re-delivering the whole file — gated by conversation +
        // mtime/md5 so it can never serve a stale or cross-chat stub.
        crate::core::read_stub_index::load();

        let cache = Arc::new(RwLock::new(SessionCache::new()));
        let bm25_cache: Arc<std::sync::Mutex<Option<crate::core::bm25_cache::Bm25CacheEntry>>> =
            Arc::new(std::sync::Mutex::new(None));

        // Register every server-local cache with the single process-wide guardian.
        // The registry stores weak targets, so closed HTTP/MCP sessions are not retained.
        let eviction_target = std::sync::Arc::new(
            crate::core::eviction_orchestrator::EvictionOrchestrator::new(
                cache.clone(),
                bm25_cache.clone(),
            ),
        );
        crate::core::eviction_orchestrator::register(&eviction_target);
        crate::core::memory_guard::start_guard(std::sync::Arc::new(
            crate::core::eviction_orchestrator::on_memory_pressure,
        ));

        let presence_root = startup
            .project_root
            .as_deref()
            .or(startup.shell_cwd.as_deref())
            .unwrap_or(".");
        let presence_agent_id =
            match crate::core::agents::AgentRegistry::register_mcp_process(presence_root) {
                Ok(agent_id) => Some(agent_id),
                Err(error) => {
                    tracing::warn!("sobre: failed to register MCP agent presence: {error}");
                    None
                }
            };

        Self {
            cache,
            session,
            call_count: Arc::new(AtomicUsize::new(0)),
            cache_ttl_secs: ttl,
            last_call: Arc::new(RwLock::new(Instant::now())),
            agent_id: Arc::new(RwLock::new(None)),
            presence_agent_id: Arc::new(RwLock::new(presence_agent_id)),
            client_name: Arc::new(RwLock::new(String::new())),
            autonomy: Arc::new(autonomy::AutonomyState::new()),
            refetch_signals: Arc::new(RwLock::new(
                crate::core::refetch_signals::RefetchSignals::default(),
            )),
            ledger: Arc::new(RwLock::new(
                crate::core::context_ledger::ContextLedger::load(),
            )),
            registry: Some(std::sync::Arc::new(
                crate::server::registry::build_registry(),
            )),
            startup_project_root: startup.project_root,
            startup_shell_cwd: startup.shell_cwd,
            peer: Arc::new(tokio::sync::RwLock::new(None)),
            has_client_roots: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            roots_resolved: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            roots_list_attempts: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            bm25_cache,
            _eviction_target: eviction_target,
        }
    }

    /// Clears the cache and saves the session if the TTL idle threshold has been exceeded.
    pub async fn check_idle_expiry(&self) {
        if self.cache_ttl_secs == 0 {
            return;
        }
        let last = *self.last_call.read().await;
        if last.elapsed().as_secs() >= self.cache_ttl_secs {
            {
                let mut session = self.session.write().await;
                let _ = session.save();
            }
            let mut cache = self.cache.write().await;
            let count = cache.clear();
            // The persisted stub index outlives the warm-cache clear, so a
            // same-conversation re-read after idle still collapses to the stub
            // via the cold fallback (#955). Flush it now for durability.
            crate::core::read_stub_index::persist();
            if count > 0 {
                tracing::info!(
                    "Cache auto-cleared after {}s idle ({count} file(s))",
                    self.cache_ttl_secs
                );
            }
        }
        *self.last_call.write().await = Instant::now();
    }

    /// Aggressive cleanup on connection drop: save session, consolidate knowledge, clear caches.
    pub async fn shutdown(&self) {
        if let Some(agent_id) = self.presence_agent_id.read().await.clone()
            && let Err(error) = crate::core::agents::AgentRegistry::finish_persistent(&agent_id)
        {
            tracing::warn!("sobre: failed to finish MCP agent presence: {error}");
        }
        {
            let mut session = self.session.write().await;
            let _ = session.save();
        }
        // Flush the persistent stub index (#955) so an unchanged re-read survives
        // this restart as a cheap stub instead of a full re-delivery.
        crate::core::read_stub_index::persist();
        {
            let mut cache = self.cache.write().await;
            let count = cache.clear();
            if count > 0 {
                tracing::info!("[shutdown] cleared {count} cached file(s)");
            }
        }
        crate::core::memory_guard::force_purge();
    }
}
