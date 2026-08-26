//! Deterministic ETPAO benchmarks for the P8/P9 optimization paths.

use serde::Deserialize;

const REQUESTS: &str = include_str!("../fixtures/etpao_requests.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    requests: Vec<EtpaoRequest>,
}

#[derive(Debug, Deserialize)]
struct EtpaoRequest {
    input_tokens: u64,
    output_tokens: u64,
    compressed_input_tokens: u64,
    routed_output_tokens: u64,
    accepted: bool,
}

fn requests() -> Vec<EtpaoRequest> {
    let fixture: Fixture = serde_json::from_str(REQUESTS).expect("valid ETPAO fixture");
    assert_eq!(
        fixture.requests.len(),
        10,
        "fixture must contain ten requests"
    );
    fixture.requests
}

fn accepted_outcomes(requests: &[EtpaoRequest]) -> u64 {
    requests.iter().filter(|request| request.accepted).count() as u64
}

fn etpao(total_tokens: u64, accepted: u64) -> f64 {
    total_tokens as f64 / accepted as f64
}

#[test]
fn baseline_etpao() {
    let requests = requests();
    let total_tokens: u64 = requests
        .iter()
        .map(|request| request.input_tokens + request.output_tokens)
        .sum();

    assert_eq!(total_tokens, 7_530);
    assert_eq!(accepted_outcomes(&requests), 10);
    assert_eq!(etpao(total_tokens, 10), 753.0);
}

#[test]
fn with_compression_etpao() {
    let requests = requests();
    let baseline: u64 = requests
        .iter()
        .map(|request| request.input_tokens + request.output_tokens)
        .sum();
    let compressed: u64 = requests
        .iter()
        .map(|request| request.compressed_input_tokens + request.output_tokens)
        .sum();

    assert_eq!(compressed, 5_620);
    assert!(compressed < baseline);
    assert!(etpao(compressed, 10) / etpao(baseline, 10) < 1.0);
}

#[test]
fn with_routing_etpao() {
    let requests = requests();
    let baseline: u64 = requests
        .iter()
        .map(|request| request.input_tokens + request.output_tokens)
        .sum();
    let routed: u64 = requests
        .iter()
        .map(|request| request.input_tokens + request.routed_output_tokens)
        .sum();

    assert_eq!(routed, 7_190);
    assert!(routed < baseline);
    assert!(etpao(routed, 10) / etpao(baseline, 10) < 1.0);
}
