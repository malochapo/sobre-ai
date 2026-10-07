//! Builds a [`ContextSnapshotV1`] from the live context stores (GL #1024).
//!
//! The builder is split into **pure projections** (`*_from_*` / `*_slice`),
//! which map a loaded store into a bounded snapshot slice and are unit-tested
//! with in-memory fixtures, and **impure anchors** (`git_anchor`,
//! `project_slice`), which read git / the filesystem. [`build`] orchestrates
//! them and finalizes the id (signing it when requested); [`create`] additionally
//! persists the snapshot and appends it to the append-only timeline.

use std::path::Path;
use std::time::Duration;

use crate::core::context_field::ContextState;
use crate::core::context_ledger::ContextLedger;
use crate::core::session::SessionState;

use super::digest::finalize_id;
use super::signing::sign_snapshot;
use super::types::{
    ContextSnapshotV1, GitAnchorV1, MAX_SNAPSHOT_LEDGER_ITEMS, MAX_SNAPSHOT_SESSION_LIST,
    SnapshotLedgerItemV1, SnapshotLedgerV1, SnapshotProjectV1, SnapshotSessionV1,
};

/// Inputs for building a snapshot.
pub struct SnapshotOptions {
    /// Project root the snapshot is anchored to.
    pub project_root: String,
    /// Sign the snapshot with the publisher keypair (else just finalize the id).
    pub sign: bool,
}

/// Build an in-memory snapshot from the current store state, finalizing its id
/// (and signing it when `opts.sign`). Does not persist anything.
pub fn build(opts: &SnapshotOptions) -> Result<ContextSnapshotV1, String> {
    let mut snap = ContextSnapshotV1::new(
        chrono::Utc::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    snap.git = git_anchor(&opts.project_root);
    snap.project = project_slice(&opts.project_root);

    snap.ledger = ledger_slice(&ContextLedger::load());
    snap.session =
        SessionState::load_latest_for_project_root(&opts.project_root).map(|s| session_slice(&s));

    snap.parent_id = super::timeline::head_id(&opts.project_root);

    if opts.sign {
        let (key, _newly_created) = super::keys::load_or_create()?;
        sign_snapshot(&mut snap, &key)?;
    } else {
        finalize_id(&mut snap)?;
    }
    Ok(snap)
}

/// Build, persist, and append the snapshot to the project's timeline.
pub fn create(opts: &SnapshotOptions) -> Result<ContextSnapshotV1, String> {
    let snap = build(opts)?;
    super::timeline::write_snapshot(&opts.project_root, &snap)?;
    Ok(snap)
}

// --- pure projections -------------------------------------------------------

fn ledger_slice(led: &ContextLedger) -> SnapshotLedgerV1 {
    let start = led.entries.len().saturating_sub(MAX_SNAPSHOT_LEDGER_ITEMS);
    let items = led.entries[start..]
        .iter()
        .map(|e| SnapshotLedgerItemV1 {
            path: e.path.clone(),
            state: state_str(e.state.unwrap_or(ContextState::Candidate)).to_string(),
            phi: e.phi,
            sent_tokens: e.sent_tokens,
            original_tokens: e.original_tokens,
        })
        .collect();
    SnapshotLedgerV1 {
        window_size: led.window_size,
        total_tokens_sent: led.total_tokens_sent,
        total_tokens_saved: led.total_tokens_saved,
        items,
    }
}

fn session_slice(s: &SessionState) -> SnapshotSessionV1 {
    SnapshotSessionV1 {
        session_id: Some(s.id.clone()),
        task: s.task.as_ref().map(|t| t.description.clone()),
        decisions: s
            .decisions
            .iter()
            .take(MAX_SNAPSHOT_SESSION_LIST)
            .map(|d| d.summary.clone())
            .collect(),
        files_touched: s
            .files_touched
            .iter()
            .take(MAX_SNAPSHOT_SESSION_LIST)
            .map(|f| f.path.clone())
            .collect(),
        progress_pct: s.task.as_ref().and_then(|t| t.progress_pct),
    }
}

// --- impure anchors ---------------------------------------------------------

fn project_slice(project_root: &str) -> SnapshotProjectV1 {
    SnapshotProjectV1 {
        root_hash: Some(crate::core::project_hash::hash_project_root(project_root)),
        identity_hash: crate::core::project_hash::project_identity(project_root)
            .map(|id| crate::core::hasher::hash_str(&id)),
    }
}

fn git_anchor(project_root: &str) -> GitAnchorV1 {
    if !crate::core::git::git_available() {
        return GitAnchorV1::default();
    }
    let root = Path::new(project_root);
    let commit = git_str(root, &["rev-parse", "HEAD"]);
    let branch = git_str(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = crate::core::git::run_git(
        &["status", "--porcelain"],
        root,
        Duration::from_secs(5),
        &[],
    )
    .is_ok_and(|o| o.success && !o.stdout.trim().is_empty());
    GitAnchorV1 {
        commit,
        branch,
        dirty,
    }
}

fn git_str(root: &Path, args: &[&str]) -> Option<String> {
    crate::core::git::run_git(args, root, Duration::from_secs(5), &[])
        .ok()
        .filter(|o| o.success)
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn state_str(s: ContextState) -> &'static str {
    match s {
        ContextState::Candidate => "candidate",
        ContextState::Included => "included",
        ContextState::Excluded => "excluded",
        ContextState::Pinned => "pinned",
        ContextState::Stale => "stale",
        ContextState::Shadowed => "shadowed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::session::{Decision, FileTouched, TaskInfo};

    #[test]
    fn session_slice_projects_task_and_lists() {
        let mut s = SessionState::new();
        s.task = Some(TaskInfo {
            description: "Implement timeline".into(),
            intent: None,
            progress_pct: Some(40),
        });
        s.decisions.push(Decision {
            summary: "JSONL append-only index".into(),
            rationale: None,
            timestamp: chrono::Utc::now(),
        });
        s.files_touched.push(FileTouched {
            path: "rust/src/core/context_snapshot/timeline.rs".into(),
            file_ref: None,
            read_count: 1,
            modified: true,
            last_mode: "full".into(),
            tokens: 100,
            stale: false,
            context_item_id: None,
            summary: None,
        });

        let slice = session_slice(&s);
        assert_eq!(slice.task.as_deref(), Some("Implement timeline"));
        assert_eq!(slice.progress_pct, Some(40));
        assert_eq!(slice.decisions, vec!["JSONL append-only index".to_string()]);
        assert_eq!(
            slice.files_touched,
            vec!["rust/src/core/context_snapshot/timeline.rs".to_string()]
        );
    }

    #[test]
    fn ledger_slice_maps_state_and_phi() {
        let mut led = ContextLedger::new();
        led.record("src/a.rs", "full", 500, 13);
        for e in &mut led.entries {
            if e.path == "src/a.rs" {
                e.phi = Some(0.9);
            }
        }
        led.set_state("src/a.rs", ContextState::Pinned);
        led.record("src/b.rs", "signatures", 800, 40);

        let slice = ledger_slice(&led);
        assert_eq!(slice.items.len(), 2);
        let a = slice.items.iter().find(|i| i.path == "src/a.rs").unwrap();
        assert_eq!(a.state, "pinned");
        assert_eq!(a.phi, Some(0.9));
        // record() marks a fresh entry as included (it just entered the window);
        // the candidate fallback only applies to entries with no recorded state.
        let b = slice.items.iter().find(|i| i.path == "src/b.rs").unwrap();
        assert_eq!(b.state, "included");
    }

    #[test]
    fn state_strings_are_snake_case() {
        assert_eq!(state_str(ContextState::Shadowed), "shadowed");
    }
}
