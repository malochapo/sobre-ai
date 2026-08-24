//! Golden contract tests for the public OCLA wire protocol.

use lean_ctx::core::ocla::types::{
    AGENT_ENVELOPE_SCHEMA_VERSION, AgentEnvelope, CANONICAL_TOKEN_ENVELOPE_SCHEMA_VERSION,
    CanonicalTokenEnvelopeV1, OclaRequestContext, TokenBalanceV1, TokenEnvelopeSurface,
    TokenFlowDirection,
};
use lean_ctx::core::ocla::wire::{
    agent_envelope_schema, canonical_envelope_schema, decode_agent_envelope, decode_envelope,
    encode_agent_envelope, encode_envelope,
};
use lean_ctx::core::ocla::wire_stream::{StreamFrame, decode_frame, encode_frame};
use serde_json::Value;

const GOLDEN_ENVELOPE: &str = include_str!("../fixtures/ocla_envelope_golden.json");
const GOLDEN_SCHEMA: &str = include_str!("../fixtures/ocla_schema_golden.json");
const LEGACY_ENVELOPE: &str = include_str!("../fixtures/envelope_v1_legacy.json");

fn golden_document() -> Value {
    serde_json::from_str(GOLDEN_ENVELOPE).expect("valid OCLA envelope fixture")
}

fn golden_agent() -> Value {
    let mut golden = golden_document()["agent"].clone();
    golden["relay_id"] = Value::String(agent_envelope().relay_id);
    golden
}

fn canonical_envelope() -> CanonicalTokenEnvelopeV1 {
    CanonicalTokenEnvelopeV1 {
        schema_version: CANONICAL_TOKEN_ENVELOPE_SCHEMA_VERSION,
        context: OclaRequestContext {
            request_id: "request-golden-001".into(),
            session_id: "session-golden-001".into(),
            agent_id: "agent-golden-001".into(),
            content_ref: "blake3:0123456789abcdef".into(),
            tenant_id: Some("tenant-golden".into()),
            trace_id: "tr-unit".into(),
        },
        surface: TokenEnvelopeSurface::Proxy,
        direction: TokenFlowDirection::Output,
        provider: "openai".into(),
        model: "gpt-5.4".into(),
        token_balance: TokenBalanceV1 {
            original_tokens: 1_234,
            materialized_tokens: 987,
            delivered_tokens: 876,
            provider_billed_tokens: 876,
        },
        route_ref: Some("route:golden-primary".into()),
        policy_ref: Some("policy:strict-v1".into()),
        idempotency_key: "request-golden-001:output".into(),
    }
}

fn agent_envelope() -> AgentEnvelope {
    let mut envelope = AgentEnvelope {
        schema_version: AGENT_ENVELOPE_SCHEMA_VERSION,
        relay_id: "agent-relay:pending".into(),
        context: OclaRequestContext {
            request_id: "agent-request-golden-001".into(),
            session_id: "agent-session-golden-001".into(),
            agent_id: "owner-agent".into(),
            content_ref: "blake3:fedcba9876543210".into(),
            tenant_id: Some("tenant-agent-golden".into()),
            trace_id: "tr-unit".into(),
        },
        from_agent_id: "owner-agent".into(),
        to_agent_id: "reviewer-agent".into(),
        capsule_ref: format!("capsule:{}", "abcdef0123456789".repeat(4)),
        budget_tokens: 4_096,
    };
    envelope.assign_relay_id().expect("assign golden relay ID");
    envelope
}

#[test]
fn canonical_envelope_matches_golden_fixture() {
    let wire = encode_envelope(&canonical_envelope()).expect("encode canonical envelope");
    let actual: Value = serde_json::from_str(&wire).expect("encoded canonical JSON");
    assert_eq!(actual, golden_document()["canonical"]);
}

#[test]
fn canonical_v1_golden_fixture_roundtrips_all_fields() {
    let fixture = golden_document()["canonical"].to_string();
    let decoded = decode_envelope(&fixture).expect("decode canonical v1 fixture");
    assert_eq!(decoded, canonical_envelope());
}

#[test]
fn agent_envelope_matches_golden_fixture() {
    let wire = encode_agent_envelope(&agent_envelope()).expect("encode agent envelope");
    let actual: Value = serde_json::from_str(&wire).expect("encoded agent JSON");
    assert_eq!(actual, golden_agent());
}

#[test]
fn agent_v1_golden_fixture_roundtrips_all_fields() {
    let fixture = golden_agent().to_string();
    let decoded = decode_agent_envelope(&fixture).expect("decode agent v1 fixture");
    assert_eq!(decoded, agent_envelope());
}

#[test]
fn canonical_schema_matches_golden_fixture() {
    let golden: Value = serde_json::from_str(GOLDEN_SCHEMA).expect("valid schema fixture");
    assert_eq!(canonical_envelope_schema(), golden);
}

#[test]
fn agent_schema_remains_self_describing() {
    let schema = agent_envelope_schema();
    assert_eq!(schema["title"], "LeanCTX AgentEnvelopeV1");
    assert_eq!(schema["properties"]["schema_version"]["const"], 1);
}

#[test]
fn external_consumer_can_read_canonical_wire_as_serde_value() {
    let wire = encode_envelope(&canonical_envelope()).expect("encode canonical envelope");
    let value: Value = serde_json::from_str(&wire).expect("external consumer parses JSON");
    let object = value.as_object().expect("wire envelope is an object");
    for field in [
        "schema_version",
        "context",
        "surface",
        "direction",
        "provider",
        "model",
        "token_balance",
        "route_ref",
        "policy_ref",
        "idempotency_key",
    ] {
        assert!(object.contains_key(field), "missing wire field: {field}");
    }
    assert_eq!(value["context"]["tenant_id"], "tenant-golden");
    assert_eq!(value["token_balance"]["delivered_tokens"], 876);
}

#[test]
fn legacy_v1_fixture_decodes_without_schema_migration() {
    let decoded = decode_envelope(LEGACY_ENVELOPE).expect("decode legacy v1 fixture");
    assert_eq!(
        decoded.schema_version,
        CANONICAL_TOKEN_ENVELOPE_SCHEMA_VERSION
    );
    assert_eq!(decoded.context.request_id, "legacy-request-001");
    assert_eq!(decoded.idempotency_key, "legacy-request-001:output");
}

#[test]
fn streaming_frames_roundtrip_in_order() {
    let frames = [
        StreamFrame::Data(Box::new(canonical_envelope())),
        StreamFrame::Heartbeat,
        StreamFrame::Cancel,
        StreamFrame::Done,
    ];
    let encoded: Vec<String> = frames
        .iter()
        .map(|frame| encode_frame(frame).expect("encode stream frame"))
        .collect();
    let decoded: Vec<StreamFrame> = encoded
        .iter()
        .map(|line| decode_frame(line).expect("decode stream frame"))
        .collect();
    assert_eq!(decoded, frames);
}
