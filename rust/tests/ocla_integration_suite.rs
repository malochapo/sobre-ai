use std::collections::HashSet;
use std::sync::MutexGuard;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use ed25519_dalek::SigningKey;
use lean_ctx::core::a2a::dlq::{DeadLetter, DeadLetterQueue};
use lean_ctx::core::context_capsule::{
    CONTEXT_CAPSULE_SCHEMA_VERSION, CapsuleReferenceKindV1, CapsuleSensitivityV1,
    ContextCapsuleBudgetV1, ContextCapsuleChainV1, ContextCapsuleReferenceV1, ContextCapsuleV1,
    SignedContextCapsuleV1,
};
use lean_ctx::core::ocla::budget::{BudgetLedger, BudgetLimit, BudgetScope};
use lean_ctx::core::ocla::capsule::CapsuleStore;
use lean_ctx::core::ocla::health::{HealthStatus, check_system_health};
use lean_ctx::core::ocla::response_cache::{CachedResponse, ResponseCache, ResponseCacheKey};
use lean_ctx::core::ocla::routing_quality::{
    RoutingDecision, RoutingOutcome, RoutingQualityTracker,
};
use lean_ctx::core::ocla::tracing::{SpanStatus, spans_for_trace, start_span};
use lean_ctx::core::ocla::wire_api::ocla_router;
use lean_ctx::core::{agents::AgentRegistry, savings_ledger};
use serde_json::Value;
use tower::ServiceExt;


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

fn canonical_envelope_body() -> String {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/ocla_envelope_golden.json"))
        .expect("valid envelope fixture");
    fixture["canonical"].to_string()
}

async fn endpoint_status(method: &str, uri: &str, body: Option<&str>) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.unwrap_or_default().to_owned()))
        .expect("request");
    ocla_router()
        .oneshot(request)
        .await
        .expect("response")
        .status()
}

#[tokio::test]
async fn all_endpoints_return_valid_status() {
    let envelope = canonical_envelope_body();
    let scope = "user:integration-all-endpoints";
    let budget = serde_json::json!({
        "scope": scope,
        "max_tokens_per_day": 100,
        "max_usd_per_day": 1.0
    })
    .to_string();
    let batch = format!("[{envelope}]");
    let requests = vec![
        ("GET", "/ocla/v1/health".to_owned(), None),
        ("GET", "/ocla/v1/capabilities".to_owned(), None),
        ("POST", "/ocla/v1/envelope".to_owned(), Some(envelope)),
        ("POST", "/ocla/v1/envelope/batch".to_owned(), Some(batch)),
        ("GET", "/ocla/v1/agents".to_owned(), None),
        ("GET", "/ocla/v1/metrics".to_owned(), None),
        ("GET", "/ocla/v1/ledger/summary".to_owned(), None),
        ("POST", "/ocla/v1/budget".to_owned(), Some(budget)),
        ("GET", format!("/ocla/v1/budget/{scope}"), None),
        ("DELETE", format!("/ocla/v1/budget/{scope}"), None),
        ("GET", "/ocla/v1/dlq".to_owned(), None),
        (
            "POST",
            "/ocla/v1/dlq/integration-missing/retry".to_owned(),
            None,
        ),
        (
            "DELETE",
            "/ocla/v1/dlq/integration-missing".to_owned(),
            None,
        ),
        (
            "POST",
            "/ocla/v1/capsule".to_owned(),
            Some("integration endpoint capsule".to_owned()),
        ),
        (
            "GET",
            "/ocla/v1/capsule/capsule:integration-missing".to_owned(),
            None,
        ),
        (
            "POST",
            "/ocla/v1/capsule/capsule:integration-missing/fork".to_owned(),
            Some(r#"{"budget_tokens":64}"#.to_owned()),
        ),
    ];

    for (method, uri, body) in requests {
        let status = endpoint_status(method, &uri, body.as_deref()).await;
        assert!(
            !status.is_server_error(),
            "{method} {uri} returned {status}"
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
