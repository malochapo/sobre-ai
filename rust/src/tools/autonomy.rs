use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::core::cache::SessionCache;
use crate::core::tokens::count_tokens;
use crate::tools::CrpMode;

/// Per-key stats for progressive search hints (`ctx_search` / `ctx_semantic_search`).
#[derive(Debug, Clone)]
pub struct SearchHistory {
    pub call_count: u32,
    pub last_call: Instant,
}

/// Tracks autonomous action state: session init, dedup, and consolidation timing.
pub struct AutonomyState {
    pub session_initialized: AtomicBool,
    pub dedup_applied: AtomicBool,
    pub last_consolidation_unix: AtomicU64,
    /// Repeated `pattern|path` keys for search tools (see [`AutonomyState::track_search`]).
    pub search_repetition: Mutex<HashMap<String, SearchHistory>>,
    /// One-shot keys for large-output hints (`ctx_shell` bytes, `ctx_read` full tokens).
    pub large_output_hints_shown: Mutex<HashSet<String>>,
}

impl Default for AutonomyState {
    fn default() -> Self {
        Self::new()
    }
}

impl AutonomyState {
    /// Creates a new autonomy state.
    pub fn new() -> Self {
        Self {
            session_initialized: AtomicBool::new(false),
            dedup_applied: AtomicBool::new(false),
            last_consolidation_unix: AtomicU64::new(0),
            search_repetition: Mutex::new(HashMap::new()),
            large_output_hints_shown: Mutex::new(HashSet::new()),
        }
    }
}

/// Cache entries before deduplication runs.
const DEDUP_THRESHOLD: usize = 8;
/// Tool calls between automatic consolidations.
pub(crate) const CONSOLIDATE_EVERY_CALLS: u32 = 25;
/// Minimum time between automatic consolidations.
const CONSOLIDATE_COOLDOWN_SECS: u64 = 120;

/// True for the call that is itself the session-start overview: `ctx_compose`
/// without a `task` returns the same overview the briefing would prepend. The
/// caller skips the pre-hook for it and marks the session as briefed (see
/// [`mark_session_briefed`]), so the agent gets the project summary once.
pub fn is_session_overview_call(
    tool_name: &str,
    args: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    tool_name == "ctx_compose"
        && args
            .and_then(|a| a.get("task"))
            .and_then(serde_json::Value::as_str)
            .is_none()
}

/// Closes the once-per-session briefing slot without emitting a briefing: the
/// call that consumes it already delivered the project overview itself.
pub fn mark_session_briefed(state: &AutonomyState) {
    state.session_initialized.store(true, Ordering::SeqCst);
}

/// Auto-preloads project context on the first tool call of a session.
pub fn session_lifecycle_pre_hook(
    state: &AutonomyState,
    cache: &mut SessionCache,
    task: Option<&str>,
    project_root: Option<&str>,
    crp_mode: CrpMode,
) -> Option<String> {
    let root = match project_root {
        Some(r) if !r.is_empty() && r != "." => r.to_string(),
        _ => return None,
    };

    // Keep the session eligible for a later retry: do not claim initialization
    // while the guardian is asking opportunistic work to back off.
    if crate::core::memory_guard::is_under_pressure() {
        return None;
    }

    if state
        .session_initialized
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return None;
    }

    let (result, usable) = if let Some(task_desc) = task {
        crate::tools::impl_preload::handle(cache, task_desc, Some(&root), crp_mode)
    } else {
        let cache_readonly = &*cache;
        crate::tools::impl_overview::handle(cache_readonly, None, Some(&root), crp_mode)
    };

    if !usable || result.trim().is_empty() {
        return None;
    }

    Some(format_briefing(&result))
}

/// Upper bound, in tokens, of the session wake-up briefing prepended to the
/// first tool result.
pub const BRIEFING_MAX_TOKENS: usize = 400;

const BRIEFING_HEADER: &str = "--- AUTO CONTEXT ---";
const BRIEFING_FOOTER: &str = "--- END AUTO CONTEXT ---";
const BRIEFING_CUT_NOTE: &str = "[… cut to fit; ctx_compose for the full overview]";

/// Wraps `body` as the session briefing, keeping whole lines from the top
/// until the briefing would exceed [`BRIEFING_MAX_TOKENS`]. A cut briefing
/// ends with a note pointing at `ctx_compose`.
fn format_briefing(body: &str) -> String {
    let wrap = |text: &str| format!("{BRIEFING_HEADER}\n{text}\n{BRIEFING_FOOTER}");
    let full = wrap(body);
    if count_tokens(&full) <= BRIEFING_MAX_TOKENS {
        return full;
    }
    let budget = BRIEFING_MAX_TOKENS
        .saturating_sub(count_tokens(&wrap(BRIEFING_CUT_NOTE)))
        .saturating_sub(1);
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0;
    for line in body.lines() {
        let cost = count_tokens(line) + 1;
        if used + cost > budget {
            break;
        }
        used += cost;
        kept.push(line);
    }
    kept.push(BRIEFING_CUT_NOTE);
    wrap(&kept.join("\n"))
}

/// Runs cache deduplication once the entry count exceeds the configured threshold.
pub fn maybe_auto_dedup(state: &AutonomyState, cache: &mut SessionCache) {
    if state
        .dedup_applied
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let entries = cache.get_all_entries();
    let threshold = DEDUP_THRESHOLD;
    if entries.len() < threshold {
        state.dedup_applied.store(false, Ordering::SeqCst);
        return;
    }

    let _ = crate::tools::impl_dedup::handle_action(cache, "apply");
}

/// Returns true if enough tool calls have elapsed to trigger auto-consolidation.
pub fn should_auto_consolidate(state: &AutonomyState, tool_calls: u32) -> bool {
    if !tool_calls.is_multiple_of(CONSOLIDATE_EVERY_CALLS) {
        return false;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let last = state.last_consolidation_unix.load(Ordering::SeqCst);
    if now.saturating_sub(last) < CONSOLIDATE_COOLDOWN_SECS {
        return false;
    }
    state.last_consolidation_unix.store(now, Ordering::SeqCst);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_briefing_is_kept_whole() {
        let b = format_briefing("PROJECT OVERVIEW 3 files\nsrc/  a.rs b.rs");
        assert!(b.starts_with(BRIEFING_HEADER) && b.ends_with(BRIEFING_FOOTER));
        assert!(b.contains("src/  a.rs b.rs"));
        assert!(!b.contains(BRIEFING_CUT_NOTE));
    }

    #[test]
    fn long_briefing_is_cut_to_the_budget_not_dropped() {
        let body: String = (0..400)
            .map(|i| format!("src/module_{i}/  file_{i}.rs helper_{i}.rs\n"))
            .collect();
        let b = format_briefing(&body);
        assert!(
            count_tokens(&b) <= BRIEFING_MAX_TOKENS,
            "{}",
            count_tokens(&b)
        );
        assert!(
            b.contains("src/module_0/"),
            "the top of the overview is kept"
        );
        assert!(b.contains(BRIEFING_CUT_NOTE));
        assert!(b.ends_with(BRIEFING_FOOTER));
    }
}
