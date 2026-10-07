//! `CONTEXT_SNAPSHOT_V1` data model (GL #1023).
//!
//! A Context Snapshot is a **git-anchored, signed, point-in-time record of the
//! context-layer state**: why each item was in the window (ledger Φ-scores +
//! item states) and which session produced it. Snapshots chain over time via [`ContextSnapshotV1::parent_id`],
//! forming the append-only timeline that powers the Context Time Machine.
//!
//! The format is intentionally a *distilled* projection of the live stores — it
//! never embeds raw transcripts (doctrine: distilled, typed, signed). Each slice
//! is bounded by the `MAX_SNAPSHOT_*` caps so a snapshot stays small and stable.

use serde::{Deserialize, Serialize};

use crate::core::contracts::CONTEXT_SNAPSHOT_V1_SCHEMA_VERSION;

/// Maximum number of ledger items embedded in a snapshot.
pub const MAX_SNAPSHOT_LEDGER_ITEMS: usize = 256;

/// Maximum number of session decisions / files embedded in the session slice.
pub const MAX_SNAPSHOT_SESSION_LIST: usize = 64;

/// A git-anchored, signed, temporal snapshot of the context-layer state.
///
/// `snapshot_id` and `signature` are excluded from the canonical body that is
/// hashed (see [`super::digest`]): the id is the BLAKE3 of that body, and the
/// signature is computed over the id. This makes the id content-addressed and
/// deterministic — the same layer state yields the same id (modulo `created_at`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshotV1 {
    /// Schema version — always [`CONTEXT_SNAPSHOT_V1_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Content-addressed identity: BLAKE3 hex of the canonical body. Empty until
    /// computed by [`super::digest::finalize_id`] or [`super::signing::sign_snapshot`].
    pub snapshot_id: String,
    /// Id of the previous snapshot in this project's timeline (chain link).
    pub parent_id: Option<String>,
    /// RFC3339 creation timestamp.
    pub created_at: String,
    /// `lean-ctx` version that produced the snapshot.
    pub lean_ctx_version: String,
    /// Git anchor: the commit / branch / dirty-state the snapshot is pinned to.
    pub git: GitAnchorV1,
    /// Project identity hashes (root + remote identity), never raw paths.
    pub project: SnapshotProjectV1,
    /// Distilled ledger slice (Φ-scores + item states), from the Context Ledger.
    pub ledger: SnapshotLedgerV1,
    /// Optional session slice (task / decisions / progress).
    pub session: Option<SnapshotSessionV1>,
    /// ed25519 signature over `snapshot_id`. `None` until signed.
    pub signature: Option<SnapshotSignatureV1>,
}

impl ContextSnapshotV1 {
    /// A new, unsigned snapshot shell with the current schema version and empty
    /// slices. Callers (the Phase-1 builder) populate the slices from live
    /// stores, then finalize the id / signature.
    #[must_use]
    pub fn new(created_at: String, lean_ctx_version: String) -> Self {
        Self {
            schema_version: CONTEXT_SNAPSHOT_V1_SCHEMA_VERSION,
            snapshot_id: String::new(),
            parent_id: None,
            created_at,
            lean_ctx_version,
            git: GitAnchorV1::default(),
            project: SnapshotProjectV1::default(),
            ledger: SnapshotLedgerV1::default(),
            session: None,
            signature: None,
        }
    }
}

/// Git anchor for a snapshot — the repository state it is pinned to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitAnchorV1 {
    /// Commit SHA the snapshot is anchored to (short or full), if in a repo.
    pub commit: Option<String>,
    /// Branch name at snapshot time.
    pub branch: Option<String>,
    /// Whether the working tree had uncommitted changes at snapshot time.
    pub dirty: bool,
}

/// Project identity — hashes only, never raw paths (privacy doctrine).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotProjectV1 {
    /// Hash of the project root path.
    pub root_hash: Option<String>,
    /// Hash of the project's remote/git identity (stable across clones).
    pub identity_hash: Option<String>,
}

/// Distilled ledger slice — why each item was in the window, with its Φ-score.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SnapshotLedgerV1 {
    pub window_size: usize,
    pub total_tokens_sent: usize,
    pub total_tokens_saved: usize,
    /// Bounded ledger items (≤ [`MAX_SNAPSHOT_LEDGER_ITEMS`]).
    pub items: Vec<SnapshotLedgerItemV1>,
}

/// One ledger entry: an item the layer decided about, with its state + Φ.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotLedgerItemV1 {
    pub path: String,
    /// Context state: `candidate` | `included` | `excluded` | `pinned` |
    /// `stale` | `shadowed`.
    pub state: String,
    pub phi: Option<f64>,
    pub sent_tokens: usize,
    pub original_tokens: usize,
}

/// Distilled session slice — the task and decisions behind the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSessionV1 {
    pub session_id: Option<String>,
    pub task: Option<String>,
    /// Bounded decision summaries (≤ [`MAX_SNAPSHOT_SESSION_LIST`]).
    pub decisions: Vec<String>,
    /// Bounded touched-file paths (≤ [`MAX_SNAPSHOT_SESSION_LIST`]).
    pub files_touched: Vec<String>,
    /// Task completion percentage `[0, 100]`, if known.
    pub progress_pct: Option<u8>,
}

/// ed25519 signature over a snapshot's `snapshot_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSignatureV1 {
    /// Always `"ed25519"`.
    pub algorithm: String,
    /// Signer's public verifying key, hex-encoded (the publisher identity).
    pub public_key: String,
    /// Signature value, hex-encoded.
    pub value: String,
}
