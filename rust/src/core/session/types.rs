use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent session state tracking task, findings, files, decisions, and stats.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionState {
    pub id: String,
    pub version: u32,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub project_root: Option<String>,
    pub shell_cwd: Option<String>,
    pub task: Option<TaskInfo>,
    pub findings: Vec<Finding>,
    pub decisions: Vec<Decision>,
    pub files_touched: Vec<FileTouched>,
    pub evidence: Vec<EvidenceRecord>,
    pub active_structured_intent: Option<crate::core::intent_engine::StructuredIntent>,
    pub stats: SessionStats,
    /// Watermark: timestamp of last auto-consolidation to prevent duplicate knowledge entries.
    pub last_consolidate_ts: Option<DateTime<Utc>>,
    /// Extra project roots for multi-root workspaces: config `search_roots`,
    /// the host's workspace folders and MCP `roots/list`. Rebuilt every time
    /// the server starts and never persisted, so removing a path from the
    /// config takes it away from a resumed session too.
    #[serde(skip)]
    pub extra_roots: Vec<String>,
    /// ACE delta playbook (#541): incremental, stable-ID checkpoint entries —
    /// grown by ctx_compress, never rewritten (anti context-collapse).
    pub playbook: super::playbook::Playbook,
    /// Last ctx_semantic_search query (#542): fallback query source for
    /// query-conditioned IB compression when no explicit task is set.
    pub last_semantic_query: Option<String>,
    /// #717: in-memory timestamp of the last flush to disk. Backs the
    /// time-based save trigger so a slow trickle of changes (below the
    /// 5-change batch) still reaches disk — the dashboard reads session
    /// JSONs and otherwise showed "idle" for the whole batch window.
    #[serde(skip)]
    pub(crate) last_flush: Option<std::time::Instant>,
}

/// Description of the current task being worked on.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskInfo {
    pub description: String,
    pub intent: Option<String>,
}

/// A discovery or observation recorded during the session.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Finding {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub summary: String,
    pub timestamp: DateTime<Utc>,
}

/// A design or implementation decision made during the session.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Decision {
    pub summary: String,
    pub rationale: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// A file that was read or modified during the session.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileTouched {
    pub path: String,
    pub file_ref: Option<String>,
    pub read_count: u32,
    pub modified: bool,
    pub last_mode: String,
    pub tokens: usize,
    pub stale: bool,
    pub context_item_id: Option<String>,
    /// One-line summary of file purpose/content (max 80 chars)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Source of an evidence record: automatic tool call or manual agent entry.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ToolCall,
    Manual,
}

/// An auditable record of a tool invocation or manual observation.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EvidenceRecord {
    pub kind: EvidenceKind,
    pub key: String,
    pub value: Option<String>,
    pub tool: Option<String>,
    pub input_md5: Option<String>,
    pub output_md5: Option<String>,
    pub agent_id: Option<String>,
    pub client_name: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Aggregate counters for the session: tool calls, token savings, cache hits.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SessionStats {
    pub total_tool_calls: u32,
    pub total_tokens_saved: u64,
    pub total_tokens_input: u64,
    pub cache_hits: u32,
    pub files_read: u32,
    pub commands_run: u32,
    pub unsaved_changes: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct LatestPointer {
    pub(crate) id: String,
}

/// Pre-serialized session data ready for background disk I/O.
/// Created by `SessionState::prepare_save()` while holding the write lock,
/// then written via `write_to_disk()` after the lock is released.
pub struct PreparedSave {
    pub(crate) dir: PathBuf,
    pub(crate) id: String,
    pub(crate) json: String,
    pub(crate) pointer_json: String,
    pub(crate) compaction_snapshot: Option<String>,
}

/// Lightweight summary of a session for listing purposes.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: u32,
    pub task: Option<String>,
    pub tool_calls: u32,
    pub tokens_saved: u64,
    /// Project root the session belongs to (GH #694): lets multi-window
    /// setups see which workspace each session serves.
    pub project_root: Option<String>,
}
