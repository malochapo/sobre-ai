use chrono::TimeZone;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

static PROOFS_COLLECTED: AtomicU64 = AtomicU64::new(0);
static PROOFS_WRITTEN: AtomicU64 = AtomicU64::new(0);
static LAST_WRITTEN_UNIX_MS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub struct ProofStatsSnapshot {
    pub collected: u64,
    pub written: u64,
    pub last_written_at: Option<String>,
}

pub fn proof_stats_snapshot() -> ProofStatsSnapshot {
    let collected = PROOFS_COLLECTED.load(Ordering::Relaxed);
    let written = PROOFS_WRITTEN.load(Ordering::Relaxed);
    let last_ms = LAST_WRITTEN_UNIX_MS.load(Ordering::Relaxed);
    let last_written_at = if last_ms == 0 {
        None
    } else {
        chrono::Utc
            .timestamp_millis_opt(last_ms as i64)
            .single()
            .map(|t| t.to_rfc3339())
    };
    ProofStatsSnapshot {
        collected,
        written,
        last_written_at,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextProofV1 {
    pub schema_version: u32,
    pub created_at: String,
    pub lean_ctx_version: String,
    pub session_id: Option<String>,
    pub project: ProjectIdentity,
    pub role: RoleIdentity,
    pub profile: ProfileIdentity,
    pub budgets: crate::core::budget_tracker::BudgetSnapshot,
    pub slo: crate::core::slo::SloSnapshot,
    pub pipeline: crate::core::pipeline::PipelineStats,
    pub verification: crate::core::output_verification::VerificationSnapshot,
    pub ledger: LedgerSummary,
    pub evidence: Vec<EvidenceReceipt>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectIdentity {
    pub project_root_hash: Option<String>,
    pub project_identity_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoleIdentity {
    pub name: String,
    pub policy_md5: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileIdentity {
    pub name: String,
    pub policy_md5: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerSummary {
    pub window_size: usize,
    pub entries: usize,
    pub total_tokens_sent: usize,
    pub total_tokens_saved: usize,
    pub compression_ratio: f64,
    pub top_files_by_sent_tokens: Vec<LedgerFileSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerFileSummary {
    pub path: String,
    pub mode: String,
    pub sent_tokens: usize,
    pub original_tokens: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceReceipt {
    pub tool: Option<String>,
    pub input_md5: Option<String>,
    pub output_md5: Option<String>,
    pub agent_id: Option<String>,
    pub client_name: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProofOptions {
    pub max_evidence: usize,
    pub max_ledger_files: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ProofSources {
    pub project_root: Option<String>,
    pub session: Option<crate::core::session::SessionState>,
    pub pipeline: Option<crate::core::pipeline::PipelineStats>,
    pub ledger: Option<crate::core::context_ledger::ContextLedger>,
}
