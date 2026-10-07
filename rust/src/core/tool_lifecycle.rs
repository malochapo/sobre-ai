//! Shared tool lifecycle — ensures CLI and MCP paths have identical side effects.
//!
//! The MCP server dispatcher handles session, ledger, usage, intent detection,
//! and knowledge consolidation inline (via in-memory state). When the daemon is
//! unavailable, CLI commands call functions here to achieve the same coverage by
//! loading/saving state from disk.
//!
//! NOTE: When the daemon IS running, CLI routes through `daemon_client` which
//! calls the MCP server — these functions are NOT called in that path.

use crate::core::context_ledger::ContextLedger;
use crate::core::intent_engine::StructuredIntent;
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

/// Record a file-read operation with full Context OS side effects.
pub fn record_file_read(
    path: &str,
    mode: &str,
    original_tokens: usize,
    output_tokens: usize,
    is_cache_hit: bool,
) {
    usage::record_compression(Surface::Cli, "read", original_tokens, output_tokens);

    if let Some(mut session) = SessionState::load_latest() {
        session.touch_file(path, None, mode, original_tokens);
        if is_cache_hit {
            session.record_cache_hit();
        }

        if session.active_structured_intent.is_none() && session.files_touched.len() >= 2 {
            let touched: Vec<String> = session
                .files_touched
                .iter()
                .map(|ft| ft.path.clone())
                .collect();
            let inferred = StructuredIntent::from_file_patterns(&touched);
            if inferred.confidence >= 0.4 {
                session.active_structured_intent = Some(inferred);
            }
        }

        let project_root = session.project_root.clone();
        let calls = session.stats.total_tool_calls;

        // Traversal edges: associate this read with the recent working set so the
        // graph learns the files this task actually touches together (#289).
        let working_set = recent_working_set(&session, path);

        let _ = session.save();

        if let Some(root) = usable_root(project_root.as_deref()) {
            crate::core::cooccurrence::record_focus_access(root, path, &working_set);
        }
        maybe_consolidate(project_root.as_deref(), calls);
    }

    // Only real files belong in the context ledger (GL #512): directory
    // overviews and synthetic paths would show up as "files" in the pressure
    // table with eviction/pin semantics that make no sense for them.
    if std::path::Path::new(path).is_file() {
        let mut ledger = ContextLedger::load();
        ledger.record(path, mode, original_tokens, output_tokens);
        ledger.save();
    }
}

/// Record a search/grep operation with full Context OS side effects.
///
/// `observed_tokens` is the measured size of the raw matched lines.
pub fn record_search(observed_tokens: usize, output_tokens: usize) {
    usage::record_compression(Surface::Cli, "grep", observed_tokens, output_tokens);

    if let Some(mut session) = SessionState::load_latest() {
        session.record_command();
        let project_root = session.project_root.clone();
        let calls = session.stats.total_tool_calls;
        let _ = session.save();

        maybe_consolidate(project_root.as_deref(), calls);
    }
}

/// Record a tree/ls operation with full Context OS side effects.
pub fn record_tree(original_tokens: usize, output_tokens: usize) {
    usage::record_compression(Surface::Cli, "ls", original_tokens, output_tokens);

    if let Some(mut session) = SessionState::load_latest() {
        session.record_command();
        let _ = session.save();
    }
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
        let project_root = session.project_root.clone();
        let calls = session.stats.total_tool_calls;
        let _ = session.save();

        if original_tokens > 0 {
            maybe_consolidate(project_root.as_deref(), calls);
        }
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
    crate::core::edit_quality::flush();
}

/// Every `CONSOLIDATE_EVERY_CALLS` CLI calls, fold the session into knowledge.
/// The MCP server also applies a time cooldown held in memory; a single-shot
/// CLI process has no such memory, so only the call-count gate applies here.
fn maybe_consolidate(project_root: Option<&str>, calls: u32) {
    let Some(root) = project_root else { return };
    if calls > 0 && calls.is_multiple_of(crate::tools::autonomy::CONSOLIDATE_EVERY_CALLS) {
        let _ = crate::core::consolidation_engine::consolidate_latest(
            root,
            crate::core::consolidation_engine::ConsolidationBudgets::default(),
        );
    }
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
    fn record_file_read_does_not_panic_without_session() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        record_file_read("/tmp/nonexistent.rs", "full", 100, 50, false);
    }

    #[test]
    fn record_search_does_not_panic_without_session() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        record_search(200, 150);
    }

    #[test]
    fn record_tree_does_not_panic_without_session() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        record_tree(100, 80);
    }

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

    #[test]
    fn cli_read_records_one_usage_event() {
        let dir = crate::core::data_dir::isolated_data_dir();
        let file = dir.path().join("sample.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        record_file_read(&file.to_string_lossy(), "full", 1000, 200, false);

        let events = usage::load();
        assert_eq!(events.len(), 1, "one CLI read is one ledger event");
        assert_eq!(events[0].str("sobre.surface"), Some("cli"));
        assert_eq!(events[0].int("sobre.tokens.original"), Some(1000));
        assert_eq!(events[0].int("sobre.tokens.sent"), Some(200));
    }
}
