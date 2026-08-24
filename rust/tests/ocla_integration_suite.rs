
use chrono::Utc;
use lean_ctx::core::a2a::dlq::{DeadLetter, DeadLetterQueue};
use lean_ctx::core::ocla::budget::{BudgetLedger, BudgetLimit, BudgetScope};
use lean_ctx::core::ocla::capsule::CapsuleStore;
use lean_ctx::core::ocla::health::check_system_health;
use lean_ctx::core::ocla::tracing::{SpanStatus, spans_for_trace, start_span};


#[test]
fn full_capsule_lifecycle() {
    let store = CapsuleStore::new();
    let data = b"integration capsule lifecycle";
    let parent_ref = store.register(data);
    let child_ref = store.fork(&parent_ref, 512).expect("capsule fork");

    assert_ne!(parent_ref, child_ref);
    assert_eq!(store.resolve(&parent_ref).expect("parent resolve"), data);
    assert_eq!(store.resolve(&child_ref).expect("child resolve"), data);
}

#[test]
fn health_reports_all_components() {
    let health = check_system_health();
    assert!(health.components.len() >= 7);
    for name in ["a2a_bus", "ledger", "budget", "dlq"] {
        assert!(
            health
                .components
                .iter()
                .any(|component| component.name == name),
            "missing health component: {name}"
        );
    }
}

#[test]
fn test_budget_blocks_over_limit() {
    let scope = BudgetScope::Org("integration-budget-block".into());
    let mut budget = BudgetLedger::new();
    budget.set_limit(BudgetLimit {
        scope: scope.clone(),
        max_tokens_per_day: 10,
        max_usd_per_day: 1.0,
    });
    budget.record_consumption(&scope, 10, 0.10);
    assert!(budget.check_budget(&scope, 1).is_err());
}

#[test]
#[ignore = "chain/budget fields not yet implemented on ContextCapsuleV1"]
fn test_capsule_fork_and_resolve() {
    // Blocked: ContextCapsuleV1 does not yet have .chain/.budget fields.
    // Re-enable once capsule chain and budget tracking are implemented.
}

#[test]
fn test_dlq_lifecycle() {
    let queue = DeadLetterQueue::new();
    queue.enqueue(DeadLetter {
        id: "integration-dead-letter".into(),
        original_message: "integration message".into(),
        target_agent: "integration-child".into(),
        error: "delivery failed".into(),
        attempts: 1,
        first_failed_at: Utc::now().to_rfc3339(),
        last_failed_at: Utc::now().to_rfc3339(),
    });
    assert_eq!(queue.peek_all().len(), 1);
    assert!(queue.dequeue("integration-dead-letter").is_some());
    assert!(queue.peek_all().is_empty());
}

#[test]
fn test_tracing_spans() {
    let trace_id = format!(
        "integration-trace-{}",
        Utc::now().timestamp_nanos_opt().unwrap()
    );
    let parent = start_span(&trace_id, "integration.parent");
    let child = start_span(&trace_id, "integration.child");
    child.set_status(SpanStatus::Ok);
    drop(child);
    drop(parent);

    let spans = spans_for_trace(&trace_id);
    assert_eq!(spans.len(), 2);
    assert!(spans.iter().all(|span| {
        span.end_ns.expect("span end") >= span.start_ns && span.trace_id == trace_id
    }));
    let child = spans
        .iter()
        .find(|span| span.operation == "integration.child")
        .expect("child span");
    let parent = spans
        .iter()
        .find(|span| span.operation == "integration.parent")
        .expect("parent span");
    assert_eq!(
        child.parent_span_id.as_deref(),
        Some(parent.span_id.as_str())
    );
}
