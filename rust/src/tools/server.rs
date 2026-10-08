use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::core::cache::SessionCache;
use crate::core::session::SessionState;
use rmcp::service::{Peer, RoleServer};

pub use crate::core::protocol::CrpMode;
// CrpMode is now defined in core::protocol to avoid reverse-dependency.
// Re-exported here for backward compatibility.

impl CrpMode {
    /// Effective CRP mode, derived from CompressionLevel.
    pub fn effective() -> Self {
        let config = crate::core::config::Config::load();
        let level = crate::core::config::CompressionLevel::effective(&config);
        let (crp_str, _) = level.to_components();
        Self::parse(crp_str).unwrap_or(Self::Off)
    }

    /// Returns true if the mode is TDD (maximum compression).
    pub fn is_tdd(&self) -> bool {
        *self == Self::Tdd
    }
}

/// Thread-safe handle to the shared file content cache.
pub type SharedCache = Arc<RwLock<SessionCache>>;

/// Central MCP server state: cache, session, metrics, and autonomy runtime.
#[derive(Clone)]
pub struct SobreServer {
    pub cache: SharedCache,
    pub session: Arc<RwLock<SessionState>>,
    pub call_count: Arc<AtomicUsize>,
    pub cache_ttl_secs: u64,
    pub last_call: Arc<RwLock<Instant>>,
    pub agent_id: Arc<RwLock<Option<String>>>,
    pub(crate) presence_agent_id: Arc<RwLock<Option<String>>>,
    pub client_name: Arc<RwLock<String>>,
    pub autonomy: Arc<super::autonomy::AutonomyState>,
    pub refetch_signals: Arc<RwLock<crate::core::refetch_signals::RefetchSignals>>,
    pub ledger: Arc<RwLock<crate::core::context_ledger::ContextLedger>>,
    pub registry: Option<Arc<crate::server::registry::ToolRegistry>>,
    pub(crate) startup_project_root: Option<String>,
    pub(crate) startup_shell_cwd: Option<String>,
    pub(crate) peer: Arc<RwLock<Option<Peer<RoleServer>>>>,
    pub(crate) has_client_roots: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) roots_resolved: Arc<std::sync::atomic::AtomicBool>,
    /// Failed `roots/list` attempts (GH #694): transient failures re-arm
    /// `roots_resolved` until a small budget is exhausted.
    pub(crate) roots_list_attempts: Arc<std::sync::atomic::AtomicU32>,
    pub(crate) bm25_cache: Arc<std::sync::Mutex<Option<crate::core::bm25_cache::Bm25CacheEntry>>>,
    pub(crate) _eviction_target: Arc<crate::core::eviction_orchestrator::EvictionOrchestrator>,
}
