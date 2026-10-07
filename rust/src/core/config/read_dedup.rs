//! Read-dedup control — whether the PostToolUse hook replaces a native Read's
//! *result* with the compact re-read stub (GL #1140, follow-up to GH #637).
//!
//! On guard hosts (Claude Code) sobre keeps the
//! PreToolUse path-swap off so the native read-before-write guard stays intact —
//! at the cost of the Read dedup savings. `PostToolUse.updatedToolOutput` restores
//! them guard-safely: the native Read has already run on the *real* path (guard
//! satisfied, first read byte-identical), and only the model-visible result of a
//! **re-read of an unchanged file** is replaced by the stub.

/// Whether the PostToolUse read-dedup may replace a re-read result for the
/// current host: only on guard hosts, where the PreToolUse redirect is off and
/// re-reads would otherwise flow at full size (#637 / GL #1140). Elsewhere the
/// PreToolUse redirect already dedups re-reads.
pub fn read_dedup_enabled() -> bool {
    super::read_redirect::host_has_read_before_write_guard()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clear every env var the guard detection reads — including the Cursor
    /// markers a Cursor agent shell exports itself (GH #720/#722) — so these
    /// tests are deterministic on any host running `cargo test`.
    fn clear_host_markers() {
        for var in [
            "CLAUDE_PROJECT_DIR",
            "CLAUDECODE",
            "CURSOR_VERSION",
            "CURSOR_PROJECT_DIR",
            "CURSOR_TRANSCRIPT_PATH",
            "CURSOR_EXTENSION_HOST_ROLE",
            "CURSOR_AGENT",
            "CURSOR_TRACE_ID",
        ] {
            crate::test_env::remove_var(var);
        }
    }

    #[test]
    fn auto_enables_only_on_guard_hosts() {
        // Inverse of read_redirect's auto: dedup where the redirect is off.
        let _lock = crate::core::data_dir::test_env_lock();
        clear_host_markers();

        assert!(
            !read_dedup_enabled(),
            "auto must stay passive off guard hosts (PreToolUse redirect dedups there)"
        );

        crate::test_env::set_var("CLAUDE_PROJECT_DIR", "/repo");
        assert!(
            read_dedup_enabled(),
            "auto must dedup under Claude Code hooks (CLAUDE_PROJECT_DIR)"
        );
        crate::test_env::remove_var("CLAUDE_PROJECT_DIR");

        // GH #722: Cursor exports CLAUDE_PROJECT_DIR for Claude-compat, but is
        // NOT a guard host — the PreToolUse redirect runs there, so the
        // PostToolUse dedup must stay passive.
        crate::test_env::set_var("CLAUDE_PROJECT_DIR", "/repo");
        crate::test_env::set_var("CURSOR_VERSION", "3.7.36");
        assert!(
            !read_dedup_enabled(),
            "Cursor must not be treated as a guard host despite CLAUDE_PROJECT_DIR"
        );
        clear_host_markers();
    }
}
