use super::*;
fn parts_for(uri: &str) -> Parts {
    Request::builder().uri(uri).body(()).unwrap().into_parts().0
}

fn add_test_marker(mut value: serde_json::Value, original_size: usize) -> (Vec<u8>, usize, usize) {
    value["sobre_touched"] = serde_json::Value::Bool(true);
    let out = serde_json::to_vec(&value).unwrap();
    let compressed_size = out.len();
    (out, original_size, compressed_size)
}

#[test]
fn upstream_is_local_detects_loopback_hosts() {
    for local in [
        "http://127.0.0.1:11434",
        "http://localhost:8080/v1",
        "http://[::1]:9999",
        "http://[::1]/v1",
        "http://0.0.0.0:4000",
        // Every address of 127.0.0.0/8 is loopback.
        "http://127.0.0.2:8080",
        "http://127.255.255.254",
        // Userinfo is not the host.
        "http://u:p@localhost:8080/v1",
        "http://user@127.0.0.1:11434",
        "http://LOCALHOST:8080",
    ] {
        assert!(
            crate::proxy::codec::upstream_is_local(local),
            "{local} must count as local"
        );
    }
    for remote in [
        "https://api.anthropic.com",
        "https://acme.services.ai.azure.com/openai",
        "https://localhost.evil.example.com", // subdomain trick ≠ local
        "http://localhost@evil.example.com",  // userinfo trick ≠ local
        "http://127.0.0.1.evil.example.com",
        "http://[::2]:9999",
        "http://128.0.0.1",
    ] {
        assert!(
            !crate::proxy::codec::upstream_is_local(remote),
            "{remote} must not be local"
        );
    }
}

#[test]
fn zstd_request_bodies_are_rewritten_and_reencoded() {
    let body = serde_json::json!({"model": "gpt-5", "input": []});
    let json = serde_json::to_vec(&body).unwrap();
    let encoded = encode_zstd(&json).unwrap();
    let parts = Request::builder()
        .uri("/v1/responses")
        .header(axum::http::header::CONTENT_ENCODING, "zstd")
        .body(())
        .unwrap()
        .into_parts()
        .0;

    let prepared = prepare_request_body(&parts, &encoded, add_test_marker).unwrap();
    assert_eq!(request_body_encoding(&parts), RequestBodyEncoding::Zstd);
    assert_eq!(prepared.request_model.as_deref(), Some("gpt-5"));
    assert!(prepared.preserve_content_encoding);
    assert!(should_forward_request_header("content-encoding", true, &[]));
    assert!(!should_forward_request_header(
        "content-encoding",
        false,
        &[]
    ));

    let decoded = zstd::decode_all(prepared.body.as_slice()).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(parsed["sobre_touched"], true);
    assert_eq!(parsed["model"], "gpt-5");
}

#[test]
fn gzip_request_bodies_are_rewritten_and_reencoded() {
    let body = serde_json::json!({"model": "gpt-5", "input": []});
    let json = serde_json::to_vec(&body).unwrap();
    let encoded = encode_gzip(&json).unwrap();
    let parts = Request::builder()
        .uri("/v1/responses")
        .header(axum::http::header::CONTENT_ENCODING, "gzip")
        .body(())
        .unwrap()
        .into_parts()
        .0;

    let prepared = prepare_request_body(&parts, &encoded, add_test_marker).unwrap();
    assert_eq!(request_body_encoding(&parts), RequestBodyEncoding::Gzip);
    assert!(prepared.preserve_content_encoding);

    let decoded = decode_gzip_bounded(&prepared.body, max_body_bytes()).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(parsed["sobre_touched"], true);
    assert_eq!(parsed["model"], "gpt-5");
}

#[test]
fn chat_requests_leave_with_only_what_compression_changed() {
    // No usage opt-in or other field is added on the way upstream.
    let _iso = crate::core::data_dir::isolated_data_dir();
    let body = serde_json::json!({"model": "gpt-5.5", "stream": true, "messages": []});
    let json = serde_json::to_vec(&body).unwrap();
    let parts = parts_for("/v1/chat/completions");

    let prepared = prepare_request_body(&parts, &json, |v, n| {
        (serde_json::to_vec(&v).unwrap(), n, n)
    })
    .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&prepared.body).unwrap();
    assert_eq!(parsed, body);
}

#[test]
fn identity_content_encoding_can_be_rewritten_as_json() {
    let parts = Request::builder()
        .uri("/v1/responses")
        .header(axum::http::header::CONTENT_ENCODING, "identity")
        .body(())
        .unwrap()
        .into_parts()
        .0;

    assert_eq!(request_body_encoding(&parts), RequestBodyEncoding::Identity);
}

#[test]
fn unknown_encoded_request_bodies_stay_passthrough() {
    let parts = Request::builder()
        .uri("/v1/responses")
        .header(axum::http::header::CONTENT_ENCODING, "br")
        .body(())
        .unwrap()
        .into_parts()
        .0;
    let body = b"not-json";

    let prepared = prepare_request_body(&parts, body, |_, _| {
        panic!("unknown encodings must not be JSON-rewritten")
    })
    .unwrap();

    assert_eq!(
        request_body_encoding(&parts),
        RequestBodyEncoding::Passthrough
    );
    assert_eq!(prepared.body, body);
    assert!(prepared.request_model.is_none());
    assert!(prepared.preserve_content_encoding);
}

#[test]
fn invalid_json_request_bodies_are_not_compression_candidates() {
    let parts = Request::builder()
        .uri("/v1/responses")
        .body(())
        .unwrap()
        .into_parts()
        .0;
    let body = b"not-json";

    let prepared = prepare_request_body(&parts, body, |_, _| {
        panic!("invalid JSON must not enter the compression pipeline")
    })
    .unwrap();

    assert_eq!(request_body_encoding(&parts), RequestBodyEncoding::Identity);
    assert_eq!(prepared.body, body);
    assert!(prepared.request_model.is_none());
    assert!(!prepared.preserve_content_encoding);
}

#[test]
fn upstream_url_preserves_subpath() {
    let base = "https://api.anthropic.com";
    let parts = parts_for("/v1/messages/count_tokens");
    assert_eq!(
        crate::proxy::codec::build_upstream_url(&parts, base, "/v1/messages"),
        "https://api.anthropic.com/v1/messages/count_tokens"
    );
}

#[test]
fn upstream_url_preserves_batches_subpath() {
    let base = "https://api.anthropic.com";
    let parts = parts_for("/v1/messages/batches/batch_123/results");
    assert_eq!(
        crate::proxy::codec::build_upstream_url(&parts, base, "/v1/messages"),
        "https://api.anthropic.com/v1/messages/batches/batch_123/results"
    );
}

#[test]
fn upstream_url_exact_path() {
    let base = "https://api.anthropic.com";
    let parts = parts_for("/v1/messages");
    assert_eq!(
        crate::proxy::codec::build_upstream_url(&parts, base, "/v1/messages"),
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn upstream_url_preserves_query_params() {
    let base = "https://api.anthropic.com";
    let parts = parts_for("/v1/messages/count_tokens?model=claude-4");
    assert_eq!(
        crate::proxy::codec::build_upstream_url(&parts, base, "/v1/messages"),
        "https://api.anthropic.com/v1/messages/count_tokens?model=claude-4"
    );
}

/// Every header a client or provider sends passes, whatever its name: the
/// proxy does not decide which ones a provider needs.
#[test]
fn headers_pass_unless_they_describe_the_connection() {
    for name in [
        "authorization",
        "x-api-key",
        "openai-project",
        "chatgpt-account-id",
        "x-codex-turn-state",
        "x-stainless-retry-count",
        "x-app",
        "x-claude-code-session-id",
        "anthropic-beta",
    ] {
        assert!(should_forward_request_header(name, false, &[]), "{name}");
    }
    for name in [
        "request-id",
        "x-should-retry",
        "retry-after-ms",
        "anthropic-ratelimit-unified-reset",
        "x-codex-primary-used-percent",
        "content-encoding",
    ] {
        assert!(is_forwarded_response_header(name, &[]), "{name}");
    }
}

#[test]
fn hop_by_hop_and_proxy_set_headers_stay() {
    let named = ["x-hop".to_string()];
    for name in [
        "connection",
        "keep-alive",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "x-hop",
        "host",
        "content-length",
        "accept-encoding",
    ] {
        assert!(!should_forward_request_header(name, true, &named), "{name}");
    }
    for name in ["connection", "transfer-encoding", "content-length", "x-hop"] {
        assert!(!is_forwarded_response_header(name, &named), "{name}");
    }
}
