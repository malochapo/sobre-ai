use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::core::autonomy_drivers::{
    AutonomyDriverDecisionV1, AutonomyDriverEventV1, AutonomyDriverKindV1, AutonomyPhaseV1,
    AutonomyVerdictV1,
};
use crate::core::cache::SessionCache;
use crate::core::config::AutonomyConfig;
use crate::core::graph_provider::GraphProvider;
use crate::core::protocol;
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
    pub config: AutonomyConfig,
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
    /// Creates a new autonomy state with config loaded from disk.
    pub fn new() -> Self {
        Self {
            session_initialized: AtomicBool::new(false),
            dedup_applied: AtomicBool::new(false),
            last_consolidation_unix: AtomicU64::new(0),
            config: AutonomyConfig::load(),
            search_repetition: Mutex::new(HashMap::new()),
            large_output_hints_shown: Mutex::new(HashSet::new()),
        }
    }

    /// Returns true if autonomous actions are enabled in configuration.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

/// Token budget and file cap of the prefetch run after a read.
const PREFETCH_BUDGET_TOKENS: usize = 4000;
const PREFETCH_MAX_FILES: usize = 3;
/// Floor of the cache-dedup threshold (`autonomy.dedup_threshold`).
const MIN_DEDUP_THRESHOLD: usize = 8;

fn autonomy_enabled_effective(state: &AutonomyState) -> bool {
    state.is_enabled()
}

fn record_event(
    phase: AutonomyPhaseV1,
    tool: &str,
    action: Option<&str>,
    decisions: Vec<AutonomyDriverDecisionV1>,
) {
    let mut store = crate::core::autonomy_drivers::AutonomyDriversV1::load();
    let ev = AutonomyDriverEventV1 {
        seq: 0,
        created_at: chrono::Utc::now().to_rfc3339(),
        phase,
        tool: tool.to_string(),
        action: action.map(std::string::ToString::to_string),
        decisions,
    };
    store.record(ev);
    let _ = store.save();
}

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
    tool_name: &str,
    cache: &mut SessionCache,
    task: Option<&str>,
    project_root: Option<&str>,
    crp_mode: CrpMode,
) -> Option<String> {
    if !autonomy_enabled_effective(state) {
        return None;
    }

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

    let mut decisions = Vec::new();

    if !state.config.auto_preload {
        decisions.push(AutonomyDriverDecisionV1 {
            driver: AutonomyDriverKindV1::Preload,
            verdict: AutonomyVerdictV1::Skip,
            reason_code: "disabled".to_string(),
            reason: "auto_preload disabled by config".to_string(),
            detail: None,
        });
        record_event(AutonomyPhaseV1::PreCall, tool_name, None, decisions);
        return None;
    }

    let chosen_tool = if task.is_some() {
        "ctx_preload"
    } else {
        "ctx_overview"
    };
    let (result, usable) = if let Some(task_desc) = task {
        crate::tools::impl_preload::handle(cache, task_desc, Some(&root), crp_mode)
    } else {
        let cache_readonly = &*cache;
        crate::tools::impl_overview::handle(cache_readonly, None, Some(&root), crp_mode)
    };

    let empty = !usable || result.trim().is_empty();
    decisions.push(AutonomyDriverDecisionV1 {
        driver: AutonomyDriverKindV1::Preload,
        verdict: AutonomyVerdictV1::Run,
        reason_code: "session_start".to_string(),
        reason: "first tool call in session".to_string(),
        detail: Some(format!("tool={chosen_tool} empty={empty}")),
    });
    record_event(AutonomyPhaseV1::PreCall, tool_name, None, decisions);

    if empty {
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

/// Appends related-file hints and silently preloads imports after a file read.
pub fn enrich_after_read(
    state: &AutonomyState,
    cache: &mut SessionCache,
    file_path: &str,
    project_root: Option<&str>,
    task: Option<&str>,
    crp_mode: CrpMode,
    minimal_overhead: bool,
) -> EnrichResult {
    let mut result = EnrichResult::default();

    if !autonomy_enabled_effective(state) {
        return result;
    }

    let root = match project_root {
        Some(r) if !r.is_empty() && r != "." => r.to_string(),
        _ => return result,
    };

    let Some(open) = crate::core::graph_provider::open_or_build(&root) else {
        return result;
    };
    let provider = &open.provider;
    if provider.file_count() == 0 {
        return result;
    }

    if state.config.auto_related {
        result.related_hint = build_related_hints(cache, file_path, provider);
    }

    if state.config.silent_preload {
        silent_preload_imports(cache, file_path, provider, &root);
    }

    if !minimal_overhead {
        let mut decisions = Vec::new();
        {
            let changed = vec![file_path.to_string()];
            let out = crate::tools::impl_prefetch::handle(
                cache,
                &root,
                task,
                Some(&changed),
                PREFETCH_BUDGET_TOKENS,
                Some(PREFETCH_MAX_FILES),
                crp_mode,
            );
            let summary = out.lines().next().unwrap_or("").trim().to_string();
            decisions.push(AutonomyDriverDecisionV1 {
                driver: AutonomyDriverKindV1::Prefetch,
                verdict: AutonomyVerdictV1::Run,
                reason_code: "after_read".to_string(),
                reason: "bounded prefetch after ctx_read".to_string(),
                detail: if summary.is_empty() {
                    None
                } else {
                    Some(summary.clone())
                },
            });
            record_event(AutonomyPhaseV1::PostRead, "ctx_read", None, decisions);
            let _ = summary;
        }
    }

    result
}

/// Output from post-read enrichment: optional related-file hints.
#[derive(Default)]
pub struct EnrichResult {
    pub related_hint: Option<String>,
}

fn build_related_hints(
    cache: &SessionCache,
    file_path: &str,
    provider: &GraphProvider,
) -> Option<String> {
    let mut related: Vec<String> = Vec::new();
    for path in provider
        .dependencies(file_path)
        .into_iter()
        .chain(provider.dependents(file_path))
    {
        if related.len() >= 3 {
            break;
        }
        if cache.get(&path).is_none() && !related.contains(&path) {
            related.push(path);
        }
    }

    if related.is_empty() {
        return None;
    }

    let hints: Vec<String> = related.iter().map(|p| protocol::shorten_path(p)).collect();

    Some(format!("[related: {}]", hints.join(", ")))
}

fn silent_preload_imports(
    cache: &mut SessionCache,
    file_path: &str,
    provider: &GraphProvider,
    project_root: &str,
) {
    let imports: Vec<String> = provider
        .dependencies(file_path)
        .into_iter()
        .take(2)
        .collect();

    let jail_root = std::path::Path::new(project_root);
    for path in imports {
        let candidate = std::path::Path::new(&path);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            jail_root.join(&path)
        };
        let Ok((jailed, warning)) = crate::core::io_boundary::jail_and_check_path(
            "autonomy:silent_preload",
            &candidate,
            jail_root,
        ) else {
            continue;
        };
        if warning.is_some() {
            continue;
        }
        let jailed_s = jailed.to_string_lossy().to_string();
        if cache.get(&jailed_s).is_some() {
            continue;
        }
        // Don't hydrate cloud placeholders during automatic import preload (#363).
        if crate::core::cloud_files::is_cloud_placeholder(&jailed) {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&jailed) {
            let tokens = count_tokens(&content);
            if tokens < 5000 {
                cache.store(&jailed_s, &content);
            }
        }
    }
}

/// Runs cache deduplication once the entry count exceeds the configured threshold.
pub fn maybe_auto_dedup(state: &AutonomyState, cache: &mut SessionCache, trigger_tool: &str) {
    if !autonomy_enabled_effective(state) {
        return;
    }

    if !state.config.auto_dedup {
        return;
    }

    if state
        .dedup_applied
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let entries = cache.get_all_entries();
    let threshold = state.config.dedup_threshold.max(MIN_DEDUP_THRESHOLD).max(1);
    if entries.len() < threshold {
        state.dedup_applied.store(false, Ordering::SeqCst);
        return;
    }

    let mut decisions = Vec::new();
    let out = crate::tools::impl_dedup::handle_action(cache, "apply");
    let summary = out.lines().next().unwrap_or("").trim().to_string();
    decisions.push(AutonomyDriverDecisionV1 {
        driver: AutonomyDriverKindV1::Dedup,
        verdict: AutonomyVerdictV1::Run,
        reason_code: "threshold_reached".to_string(),
        reason: format!("cache entries >= {threshold}"),
        detail: if summary.is_empty() {
            None
        } else {
            Some(summary)
        },
    });
    record_event(AutonomyPhaseV1::PostRead, trigger_tool, None, decisions);
}

/// Returns true if enough tool calls have elapsed to trigger auto-consolidation.
pub fn should_auto_consolidate(state: &AutonomyState, tool_calls: u32) -> bool {
    if !state.is_enabled() || !state.config.auto_consolidate {
        return false;
    }
    let every = state.config.consolidate_every_calls.max(1);
    if !tool_calls.is_multiple_of(every) {
        return false;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let last = state.last_consolidation_unix.load(Ordering::SeqCst);
    if now.saturating_sub(last) < state.config.consolidate_cooldown_secs {
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
