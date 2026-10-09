//! Learning gotchas from real shell outcomes.
//!
//! `shell::exec` reports every finished build/test/run command here: a
//! failure pushes a pending error, and the next green run of the same command
//! base correlates the fix into a persisted [`Gotcha`], with no agent action.
//!
//! Each outcome loads the project's `gotchas.json`, learns and saves it under
//! the project's knowledge lock. Pending errors are persisted with the rest,
//! so a fail→fix spanning two processes (the MCP server, `sobre -c`)
//! correlates, and no process keeps a copy that would overwrite what another
//! one saved.
//!
//! [`Gotcha`]: super::Gotcha

use std::sync::OnceLock;

use super::GotchaStore;

impl GotchaStore {
    /// Learn from one finished command: push a pending error on failure, or
    /// correlate a fix when a prior pending of the same command base now passes.
    ///
    /// Returns `true` when the store changed and the caller should persist.
    /// Pure — no globals, no I/O — so the correlation logic is unit-testable
    /// in isolation.
    pub fn learn_from_shell(
        &mut self,
        command: &str,
        output: &str,
        exit_code: i32,
        files_touched: &[String],
        session_id: &str,
    ) -> bool {
        if self.detect_error(output, command, exit_code, files_touched, session_id) {
            // detect_error appended to error_log and bumped stats — both persist.
            return true;
        }
        self.try_resolve_pending(command, files_touched, session_id)
            .is_some()
    }
}

/// Stable per-process session id. Each MCP server run / CLI invocation counts as one
/// "session" for cross-session correlation — semantically correct without
/// coupling the shell layer to the MCP session store.
fn process_session_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| format!("proc-{}", std::process::id()))
}

/// Shell-layer hook: learn from a finished command's outcome.
///
/// Gated to build/test/run command families via `is_correlatable_command`
/// so ordinary commands never touch disk. Resolves the project root and a
/// stable session id internally, keeping the call site (a single line in
/// `shell::exec`) minimal. Never fails the command: an error is dropped.
pub fn record_shell_outcome(command: &str, output: &str, exit_code: i32) {
    if !super::detect::is_correlatable_command(command) {
        return;
    }

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let project_root = crate::core::protocol::detect_project_root_or_cwd(&cwd);
    let _ = GotchaStore::update(&project_root, |store| {
        store.learn_from_shell(command, output, exit_code, &[], process_session_id())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gotcha_tracker::GotchaSource;

    #[test]
    fn learn_records_error_then_correlates_fix() {
        let mut store = GotchaStore::new("h");

        let changed = store.learn_from_shell(
            "cargo build",
            "error[E0507]: cannot move out of `self.name`",
            1,
            &[],
            "s1",
        );
        assert!(changed, "failing build must record a pending error");
        assert_eq!(store.pending_errors.len(), 1);
        assert!(store.gotchas.is_empty(), "no gotcha until a fix correlates");

        let fixed = store.learn_from_shell(
            "cargo build --release",
            "Finished `release` profile [optimized] target(s)",
            0,
            &["src/main.rs".into()],
            "s1",
        );
        assert!(fixed, "green run of same base must correlate the fix");
        assert_eq!(store.gotchas.len(), 1);
        assert!(matches!(
            store.gotchas[0].source,
            GotchaSource::AutoDetected { .. }
        ));
        assert_eq!(store.pending_errors.len(), 0, "pending consumed by the fix");
    }

    /// A gotcha another writer saved between two shell outcomes survives the
    /// second one: each outcome starts from what is on disk.
    #[test]
    fn shell_outcomes_keep_gotchas_saved_in_between() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let root = "/tmp/sobre-gotcha-no-lost-update";
        let fail = |store: &mut GotchaStore| {
            store.learn_from_shell(
                "cargo build",
                "error[E0382]: borrow of moved value",
                1,
                &[],
                "s1",
            )
        };
        assert!(GotchaStore::update(root, fail).unwrap());

        let mut other = GotchaStore::load(root);
        other.report_gotcha("flaky port", "use port 0", "convention", "warning", "s2");
        other.save(root).unwrap();

        assert!(GotchaStore::update(root, fail).unwrap());
        let store = GotchaStore::load(root);
        assert!(store.gotchas.iter().any(|g| g.trigger == "flaky port"));
    }

    #[test]
    fn learn_ignores_clean_runs() {
        let mut store = GotchaStore::new("h");
        let changed = store.learn_from_shell("cargo build", "Finished `dev` profile", 0, &[], "s1");
        assert!(
            !changed,
            "a green run with no pending error changes nothing"
        );
        assert!(store.gotchas.is_empty());
        assert!(store.pending_errors.is_empty());
    }
}
