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

fn profile_autonomy() -> crate::core::profiles::ProfileAutonomy {
    crate::core::profiles::active_profile().autonomy
}

fn autonomy_enabled_effective(state: &AutonomyState) -> bool {
    state.is_enabled() && profile_autonomy().enabled_effective()
}

fn policy_allows(tool: &str) -> Result<(), (String, String)> {
    let policy = crate::core::degradation_policy::evaluate_v1_for_tool(tool, None);
    match policy.decision.verdict {
        crate::core::degradation_policy::DegradationVerdictV1::Ok
        | crate::core::degradation_policy::DegradationVerdictV1::Warn => Ok(()),
        crate::core::degradation_policy::DegradationVerdictV1::Throttle
        | crate::core::degradation_policy::DegradationVerdictV1::Block => {
            Err((policy.decision.reason_code, policy.decision.reason))
        }
    }
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
        role: crate::core::roles::active_role_name(),
        profile: crate::core::profiles::active_profile_name(),
        tool: tool.to_string(),
        action: action.map(std::string::ToString::to_string),
        decisions,
    };
    store.record(ev);
    let _ = store.save();
}

/// True for the call that is itself the session-start overview: `ctx_compose`
/// without a `task` returns the same overview the briefing would prepend. The
/// caller skips the pre-hook for it, so the once-per-session slot stays open
/// for the next call.
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

    let prof = profile_autonomy();
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

    if !state.config.auto_preload || !prof.auto_preload_effective() {
        decisions.push(AutonomyDriverDecisionV1 {
            driver: AutonomyDriverKindV1::Preload,
            verdict: AutonomyVerdictV1::Skip,
            reason_code: "disabled".to_string(),
            reason: "auto_preload disabled by config/profile".to_string(),
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
    if let Err((code, reason)) = policy_allows(chosen_tool) {
        decisions.push(AutonomyDriverDecisionV1 {
            driver: AutonomyDriverKindV1::Preload,
            verdict: AutonomyVerdictV1::Skip,
            reason_code: code,
            reason,
            detail: Some("policy guard (budget/slo)".to_string()),
        });
        record_event(AutonomyPhaseV1::PreCall, tool_name, None, decisions);
        return None;
    }

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

    Some(format!(
        "--- AUTO CONTEXT ---\n{result}\n--- END AUTO CONTEXT ---"
    ))
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

    let prof = profile_autonomy();
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

    if state.config.auto_related && prof.auto_related_effective() {
        result.related_hint = build_related_hints(cache, file_path, provider);
    }

    if state.config.silent_preload && prof.silent_preload_effective() {
        silent_preload_imports(cache, file_path, provider, &root);
    }

    if !minimal_overhead && prof.auto_prefetch_effective() {
        let mut decisions = Vec::new();
        if let Err((code, reason)) = policy_allows("ctx_prefetch") {
            decisions.push(AutonomyDriverDecisionV1 {
                driver: AutonomyDriverKindV1::Prefetch,
                verdict: AutonomyVerdictV1::Skip,
                reason_code: code,
                reason,
                detail: Some("policy guard (budget/slo)".to_string()),
            });
            record_event(AutonomyPhaseV1::PostRead, "ctx_read", None, decisions);
        } else {
            let changed = vec![file_path.to_string()];
            let out = crate::tools::impl_prefetch::handle(
                cache,
                &root,
                task,
                Some(&changed),
                prof.prefetch_budget_tokens_effective(),
                Some(prof.prefetch_max_files_effective()),
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

    let prof = profile_autonomy();
    if !state.config.auto_dedup || !prof.auto_dedup_effective() {
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
    let threshold = state
        .config
        .dedup_threshold
        .max(prof.dedup_threshold_effective())
        .max(1);
    if entries.len() < threshold {
        state.dedup_applied.store(false, Ordering::SeqCst);
        return;
    }

    let mut decisions = Vec::new();
    if let Err((code, reason)) = policy_allows("ctx_dedup") {
        decisions.push(AutonomyDriverDecisionV1 {
            driver: AutonomyDriverKindV1::Dedup,
            verdict: AutonomyVerdictV1::Skip,
            reason_code: code,
            reason,
            detail: Some("policy guard (budget/slo)".to_string()),
        });
        record_event(AutonomyPhaseV1::PostRead, trigger_tool, None, decisions);
        state.dedup_applied.store(false, Ordering::SeqCst);
        return;
    }

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
