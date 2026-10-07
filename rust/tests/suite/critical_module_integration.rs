// Integration tests for modules identified as critical during the architecture audit.
// Covers: degradation_policy, gotcha_tracker/learn, cache CCR.

// ---------------------------------------------------------------------------
// gotcha_tracker/learn: extract learnings from resolved gotchas
// ---------------------------------------------------------------------------
#[test]
fn learn_extracts_high_confidence_gotchas() {
    use lean_ctx::core::gotcha_tracker::learn::extract_learnings;
    use lean_ctx::core::gotcha_tracker::{
        Gotcha, GotchaCategory, GotchaSeverity, GotchaSource, GotchaStats, GotchaStore,
    };

    let mut g = Gotcha::new(
        GotchaCategory::Build,
        GotchaSeverity::Warning,
        "cargo build fails with missing feature",
        "Add --all-features flag",
        GotchaSource::AutoDetected {
            command: "cargo build".into(),
            exit_code: 1,
        },
        "sess-1",
    );
    g.confidence = 0.8;
    g.occurrences = 3;
    g.session_ids = vec!["a".into(), "b".into(), "c".into()];

    let store = GotchaStore {
        project_hash: "test".into(),
        gotchas: vec![g],
        error_log: vec![],
        stats: GotchaStats::default(),
        updated_at: chrono::Utc::now(),
        pending_errors: vec![],
    };

    let learnings = extract_learnings(&store);
    assert_eq!(learnings.len(), 1);
    assert!(learnings[0].resolution.contains("--all-features"));
}

#[test]
fn learn_filters_low_confidence() {
    use lean_ctx::core::gotcha_tracker::learn::extract_learnings;
    use lean_ctx::core::gotcha_tracker::*;

    let mut g = Gotcha::new(
        GotchaCategory::Build,
        GotchaSeverity::Info,
        "some flaky thing",
        "retry",
        GotchaSource::AutoDetected {
            command: "make".into(),
            exit_code: 1,
        },
        "sess-1",
    );
    g.confidence = 0.3;
    g.occurrences = 1;

    let store = GotchaStore {
        project_hash: "test".into(),
        gotchas: vec![g],
        error_log: vec![],
        stats: GotchaStats::default(),
        updated_at: chrono::Utc::now(),
        pending_errors: vec![],
    };

    let learnings = extract_learnings(&store);
    assert!(
        learnings.is_empty(),
        "low confidence gotchas should be filtered"
    );
}

#[test]
fn cache_get_full_content_returns_stored_content() {
    use lean_ctx::core::cache::SessionCache;

    let mut cache = SessionCache::new();
    cache.store("/test/file.rs", "fn main() {}");
    let content = cache.get_full_content("/test/file.rs");
    assert!(content.is_some());
    assert!(content.unwrap().contains("fn main"));
}

#[test]
fn cache_get_full_content_returns_none_for_missing() {
    use lean_ctx::core::cache::SessionCache;

    let cache = SessionCache::new();
    let missing = cache.get_full_content("/nonexistent.rs");
    assert!(missing.is_none());
}
