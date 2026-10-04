//! Feature config sections (autonomy, providers, loop-detection, updates,
//! boundary-policy, secret-detection, gain).
//! Split out of `schema/mod.rs`; `use super::*` re-imports helpers + `SectionSchema`.

#[allow(clippy::wildcard_imports)]
use super::*;
use std::collections::BTreeMap;

pub(super) fn build(sections: &mut BTreeMap<String, SectionSchema>) {
    let cfg = crate::core::config::Config::default();
    let mut autonomy = BTreeMap::new();
    autonomy.insert(
        "enabled".into(),
        key(
            "bool",
            serde_json::json!(cfg.autonomy.enabled),
            "Enable autonomous background behaviors",
        ),
    );
    autonomy.insert(
        "auto_preload".into(),
        key(
            "bool",
            serde_json::json!(cfg.autonomy.auto_preload),
            "Auto-preload related files on first read",
        ),
    );
    autonomy.insert(
        "auto_dedup".into(),
        key(
            "bool",
            serde_json::json!(cfg.autonomy.auto_dedup),
            "Auto-deduplicate repeated reads",
        ),
    );
    autonomy.insert(
        "auto_related".into(),
        key(
            "bool",
            serde_json::json!(cfg.autonomy.auto_related),
            "Auto-load graph-related files",
        ),
    );
    autonomy.insert(
        "auto_consolidate".into(),
        key(
            "bool",
            serde_json::json!(cfg.autonomy.auto_consolidate),
            "Auto-consolidate knowledge periodically",
        ),
    );
    autonomy.insert(
        "silent_preload".into(),
        key(
            "bool",
            serde_json::json!(cfg.autonomy.silent_preload),
            "Suppress preload notifications in output",
        ),
    );
    autonomy.insert(
        "dedup_threshold".into(),
        key(
            "usize",
            serde_json::json!(cfg.autonomy.dedup_threshold),
            "Number of repeated reads before dedup triggers",
        ),
    );
    autonomy.insert(
        "consolidate_every_calls".into(),
        key(
            "u32",
            serde_json::json!(cfg.autonomy.consolidate_every_calls),
            "Consolidate knowledge every N tool calls",
        ),
    );
    autonomy.insert(
        "consolidate_cooldown_secs".into(),
        key(
            "u64",
            serde_json::json!(cfg.autonomy.consolidate_cooldown_secs),
            "Minimum seconds between consolidation runs",
        ),
    );
    autonomy.insert(
        "cognition_loop_enabled".into(),
        key_with_env(
            "bool",
            serde_json::json!(cfg.autonomy.cognition_loop_enabled),
            "Enable the background cognition loop (periodic knowledge consolidation)",
            "LEAN_CTX_COGNITION_LOOP_ENABLED",
        ),
    );
    autonomy.insert(
        "cognition_loop_interval_secs".into(),
        key_with_env(
            "u64",
            serde_json::json!(cfg.autonomy.cognition_loop_interval_secs),
            "Seconds between cognition loop iterations",
            "LEAN_CTX_COGNITION_LOOP_INTERVAL_SECS",
        ),
    );
    autonomy.insert(
        "cognition_loop_max_steps".into(),
        key_with_env(
            "u8",
            serde_json::json!(cfg.autonomy.cognition_loop_max_steps),
            "Maximum steps per cognition loop iteration (>= 9 enables observation synthesis)",
            "LEAN_CTX_COGNITION_LOOP_MAX_STEPS",
        ),
    );
    autonomy.insert(
        "cognition_synthesis_min_cluster".into(),
        key_with_env(
            "usize",
            serde_json::json!(cfg.autonomy.cognition_synthesis_min_cluster),
            "Minimum facts per entity before observation synthesis writes a summary (needs cognition_loop_max_steps >= 9)",
            "LEAN_CTX_COGNITION_SYNTHESIS_MIN_CLUSTER",
        ),
    );
    sections.insert(
        "autonomy".into(),
        SectionSchema {
            description: "Controls autonomous background behaviors (preload, dedup, consolidation)"
                .into(),
            keys: autonomy,
        },
    );

    let mut loop_det = BTreeMap::new();
    loop_det.insert(
        "normal_threshold".into(),
        key(
            "u32",
            serde_json::json!(cfg.loop_detection.normal_threshold),
            "Repetitions before reducing output",
        ),
    );
    loop_det.insert(
        "reduced_threshold".into(),
        key(
            "u32",
            serde_json::json!(cfg.loop_detection.reduced_threshold),
            "Repetitions before further reducing output",
        ),
    );
    loop_det.insert(
        "blocked_threshold".into(),
        key(
            "u32",
            serde_json::json!(cfg.loop_detection.blocked_threshold),
            "Repetitions before blocking. 0 = disabled",
        ),
    );
    loop_det.insert(
        "window_secs".into(),
        key(
            "u64",
            serde_json::json!(cfg.loop_detection.window_secs),
            "Time window in seconds for loop detection",
        ),
    );
    loop_det.insert(
        "search_group_limit".into(),
        key(
            "u32",
            serde_json::json!(cfg.loop_detection.search_group_limit),
            "Maximum unique searches within a loop window",
        ),
    );
    loop_det.insert(
            "tool_total_limits".into(),
            key(
                "table",
                serde_json::json!({"ctx_read": 100, "ctx_search": 80, "ctx_shell": 50, "ctx_semantic_search": 60}),
                "Per-tool total call limits within a session. Keys are tool names, values are max calls",
            ),
        );
    sections.insert(
        "loop_detection".into(),
        SectionSchema {
            description: "Loop detection settings for preventing repeated identical tool calls"
                .into(),
            keys: loop_det,
        },
    );

    let mut context = BTreeMap::new();
    context.insert(
        "budget_tokens".into(),
        key_with_env(
            "usize",
            serde_json::json!(cfg.context.budget_tokens),
            "Fixed per-session context budget (tool schemas + MCP instructions + auto-loaded rules + wakeup briefing). `doctor overhead` warns past this; `doctor overhead --gate` exits non-zero for CI. 0 disables the warning",
            "LEAN_CTX_CONTEXT_BUDGET_TOKENS",
        ),
    );
    context.insert(
        "proactive_expansion".into(),
        key(
            "bool",
            serde_json::json!(cfg.context.proactive_expansion),
            "Inject relevant CCR archives into later tool responses",
        ),
    );
    context.insert(
        "proactive_expansion_budget_tokens".into(),
        key(
            "usize",
            serde_json::json!(cfg.context.proactive_expansion_budget_tokens),
            "Maximum proactive archive tokens per tool response",
        ),
    );
    context.insert(
        "proactive_expansion_threshold".into(),
        key(
            "f64",
            serde_json::json!(cfg.context.proactive_expansion_threshold),
            "Minimum normalized BM25 score for proactive expansion",
        ),
    );
    context.insert(
        "proactive_expansion_max_age_secs".into(),
        key(
            "u64",
            serde_json::json!(cfg.context.proactive_expansion_max_age_secs),
            "Maximum age of CCR content eligible for proactive expansion",
        ),
    );
    sections.insert(
        "context".into(),
        SectionSchema {
            description: "Fixed-context budget accounting (#964)".into(),
            keys: context,
        },
    );

    let mut boundary = BTreeMap::new();
    boundary.insert(
        "universal_gotchas_enabled".into(),
        key(
            "bool",
            serde_json::json!(cfg.boundary_policy.universal_gotchas_enabled),
            "Load universal (cross-project) gotchas",
        ),
    );
    sections.insert(
        "boundary_policy".into(),
        SectionSchema {
            description: "Cross-project knowledge sharing".into(),
            keys: boundary,
        },
    );

    let mut secret_det = BTreeMap::new();
    secret_det.insert(
        "enabled".into(),
        key(
            "bool",
            serde_json::json!(cfg.secret_detection.enabled),
            "Enable secret/credential detection in tool outputs",
        ),
    );
    secret_det.insert(
        "redact".into(),
        key(
            "bool",
            serde_json::json!(cfg.secret_detection.redact),
            "Redact detected secrets from output",
        ),
    );
    secret_det.insert(
        "custom_patterns".into(),
        key(
            "array",
            serde_json::json!(cfg.secret_detection.custom_patterns),
            "Additional regex patterns to detect as secrets",
        ),
    );
    secret_det.insert(
        "exclude_patterns".into(),
        key(
            "array",
            serde_json::json!(cfg.secret_detection.exclude_patterns),
            "Subtractive allowlist: matches covered by these regexes are never reported or redacted (#718)",
        ),
    );
    sections.insert(
        "secret_detection".into(),
        SectionSchema {
            description: "Secret/credential detection and redaction settings".into(),
            keys: secret_det,
        },
    );

    let mut sensitivity = BTreeMap::new();
    sensitivity.insert(
        "enabled".into(),
        key(
            "bool",
            serde_json::json!(cfg.sensitivity.enabled),
            "Enable the per-item sensitivity policy floor (no-op when false)",
        ),
    );
    sensitivity.insert(
        "policy_floor".into(),
        key(
            "string",
            serde_json::json!(cfg.sensitivity.policy_floor.as_str()),
            "Block items at/above this level: public|internal|confidential|secret",
        ),
    );
    sensitivity.insert(
        "action".into(),
        key(
            "string",
            serde_json::json!(cfg.sensitivity.action.as_str()),
            "How to enforce the floor: redact (mask spans) or drop (withhold item)",
        ),
    );
    sections.insert(
        "sensitivity".into(),
        SectionSchema {
            description: "Per-item sensitivity model with a uniform policy floor (#212)".into(),
            keys: sensitivity,
        },
    );

    let mut gain = BTreeMap::new();
    gain.insert(
            "auto_publish".into(),
            key(
                "bool",
                serde_json::json!(cfg.gain.auto_publish),
                "Automatically (re)publish your Wrapped recap when you run `lean-ctx gain` (opt-in, off by default; throttled and sends only an aggregate payload)",
            ),
        );
    gain.insert(
        "leaderboard".into(),
        key(
            "bool",
            serde_json::json!(cfg.gain.leaderboard),
            "When auto-publishing, also list the card on the public opt-in leaderboard",
        ),
    );
    gain.insert(
        "display_name".into(),
        key(
            "string?",
            serde_json::json!(cfg.gain.display_name),
            "Optional display name shown on your published card / leaderboard entry",
        ),
    );
    gain.insert(
        "auto_publish_interval_hours".into(),
        key(
            "u64",
            serde_json::json!(cfg.gain.auto_publish_interval_hours),
            "Minimum hours between automatic publishes (throttle; default 24)",
        ),
    );
    gain.insert(
        "last_auto_publish".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Timestamp of the last automatic publish (written by lean-ctx for throttling — not meant to be edited)",
        ),
    );
    sections.insert(
        "gain".into(),
        SectionSchema {
            description: "Token-savings recap publishing (gain --publish / auto-publish)".into(),
            keys: gain,
        },
    );
}
