//! Shared tool lifecycle — gives the `sobre -c` shell wrapper the session,
//! usage and consolidation side effects the MCP server applies inline, by
//! loading and saving state from disk.

use crate::core::session::SessionState;
use crate::core::usage::{self, Surface};

/// How many recently-touched files form the "working set" a new read is
/// associated with for traversal (co-access) edges (#289). Small, so the signal
/// stays local to what the agent is actively juggling.
const TRAVERSAL_WINDOW: usize = 6;

/// Recent distinct file paths (excluding `current`), most-recent first, capped
/// to the traversal window — the working set a new read co-occurs with.
pub(crate) fn recent_working_set(session: &SessionState, current: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in session.files_touched.iter().rev() {
        if f.path == current || out.contains(&f.path) {
            continue;
        }
        out.push(f.path.clone());
        if out.len() >= TRAVERSAL_WINDOW {
            break;
        }
    }
    out
}

/// Whether `root` is a usable project root for repo-relative normalization.
pub(crate) fn usable_root(root: Option<&str>) -> Option<&str> {
    root.filter(|r| !r.trim().is_empty() && *r != ".")
}

/// Record a shell command with full Context OS side effects. The usage ledger
/// files the compression under the command's program name (`cargo`, `git`).
pub fn record_shell_command(command: &str, original_tokens: usize, output_tokens: usize) {
    usage::record_compression(
        Surface::Shell,
        program_name(command),
        original_tokens,
        output_tokens,
    );

    if let Some(mut session) = SessionState::load_latest() {
        session.record_command();
        let _ = session.save();
    }
}

/// First word of a command line, without its directory: `/usr/bin/git log`
/// gives `git`.
pub(crate) fn program_name(command: &str) -> &str {
    let first = command.split_whitespace().next().unwrap_or("");
    first.rsplit(['/', '\\']).next().unwrap_or(first)
}

/// Flush every buffered store to disk.
///
/// The long-lived MCP daemon flushes these once at shutdown
/// (`cli/dispatch/server.rs`). Single-shot CLI commands exit immediately, so
/// without this their buffered writes are lost the moment the process ends.
/// The usage ledger needs no flush: every event is appended as it happens.
pub fn flush_all() {
    crate::core::auto_mode_resolver::flush_sources();
}

#[cfg(test)]
mod tests {
    use super::*;

    // The record_* paths now drive process-global telemetry sinks (mode
    // predictor buffer, anomaly singleton) and read the data-dir env (#550), so
    // every test here takes the shared isolation lock to serialize that state
    // and keep its disk writes inside a throwaway dir.

    // An unreadable path falls back to the caller's count, so the ratio is
    // exactly delivered/original.

    #[test]
    fn record_shell_does_not_panic_without_session() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        record_shell_command("cargo test", 500, 200);
    }

    #[test]
    fn program_name_is_the_first_word_without_directory() {
        assert_eq!(program_name("/usr/bin/git log -5"), "git");
        assert_eq!(program_name("cargo test"), "cargo");
        assert_eq!(program_name(""), "");
    }

    #[test]
    fn flush_all_is_idempotent_and_safe_without_state() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        // Empty buffers: flushing must be a harmless no-op, and calling it twice
        // (e.g. a CLI arm followed by an atexit path) must never panic.
        flush_all();
        flush_all();
    }
}
