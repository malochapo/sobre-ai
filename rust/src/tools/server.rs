use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::core::cache::SessionCache;
use crate::core::session::SessionState;
use rmcp::service::{Peer, RoleServer};

// Defined in core::protocol, which tools must not depend back on.
pub use crate::core::protocol::CrpMode;

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
    pub last_call: Arc<RwLock<Instant>>,
    pub autonomy: Arc<super::autonomy::AutonomyState>,
    pub refetch_signals: Arc<RwLock<crate::core::refetch_signals::RefetchSignals>>,
    pub registry: Arc<crate::server::registry::ToolRegistry>,
    pub(crate) startup_project_root: Option<String>,
    pub(crate) startup_shell_cwd: Option<String>,
    pub(crate) peer: Arc<RwLock<Option<Peer<RoleServer>>>>,
    pub(crate) has_client_roots: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) roots_resolved: Arc<std::sync::atomic::AtomicBool>,
    /// Held while `roots/list` is asked, so the calls that arrive meanwhile
    /// wait for its answer.
    pub(crate) roots_gate: Arc<tokio::sync::Mutex<()>>,
    /// Failed `roots/list` attempts: transient failures re-arm
    /// `roots_resolved` until a small budget is exhausted.
    pub(crate) roots_list_attempts: Arc<std::sync::atomic::AtomicU32>,
    pub(crate) bm25_cache: Arc<std::sync::Mutex<Option<crate::core::bm25_cache::Bm25CacheEntry>>>,
    pub(crate) _eviction_target: Arc<crate::core::eviction_orchestrator::EvictionOrchestrator>,
}
