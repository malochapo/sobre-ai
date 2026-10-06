//! Split from `proxy/mod.rs` (#660 LOC gate): `stats_tests`.

use super::*;
use std::sync::atomic::Ordering;

#[test]
fn compression_ratio_includes_uncompressed_requests() {
    let stats = ProxyStats::default();

    stats.record_request(1_000, 500);
    stats.record_request(1_000, 1_000);

    assert_eq!(stats.requests_total.load(Ordering::Relaxed), 2);
    assert_eq!(stats.requests_compressed.load(Ordering::Relaxed), 1);
    assert_eq!(stats.tokens_saved.load(Ordering::Relaxed), 125);
    assert_eq!(stats.compression_ratio(), 25.0);
}

#[test]
fn expanded_requests_count_as_zero_savings() {
    let stats = ProxyStats::default();

    stats.record_request(1_000, 1_500);

    assert_eq!(stats.requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(stats.requests_compressed.load(Ordering::Relaxed), 0);
    assert_eq!(stats.tokens_saved.load(Ordering::Relaxed), 0);
    assert_eq!(stats.compression_ratio(), 0.0);
}

#[test]
fn provider_stats_are_separate() {
    let stats = ProxyStats::default();

    stats.record_provider_request("OpenAI", 1_000, 500);
    stats.record_provider_request("Gemini", 2_000, 1_000);

    assert_eq!(stats.requests_total.load(Ordering::Relaxed), 2);
    assert_eq!(stats.openai.requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(stats.gemini.requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(stats.openai.tokens_saved.load(Ordering::Relaxed), 125);
    assert_eq!(stats.gemini.tokens_saved.load(Ordering::Relaxed), 250);
    assert_eq!(stats.openai.compression_ratio(), 50.0);
    assert_eq!(stats.gemini.compression_ratio(), 50.0);
}

#[test]
fn unlabelled_requests_do_not_count_as_gemini() {
    let stats = ProxyStats::default();

    stats.record_request(1_000, 500);

    assert_eq!(stats.requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(stats.gemini.requests_total.load(Ordering::Relaxed), 0);
}

#[test]
fn unknown_label_is_not_recorded_to_any_bucket() {
    let stats = ProxyStats::default();

    stats.record_provider_request("Mystery", 1_000, 500);

    // Totals still count it; no per-upstream bucket is touched.
    assert_eq!(stats.requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(stats.anthropic.requests_total.load(Ordering::Relaxed), 0);
    assert_eq!(stats.openai.requests_total.load(Ordering::Relaxed), 0);
    assert_eq!(stats.gemini.requests_total.load(Ordering::Relaxed), 0);
}
