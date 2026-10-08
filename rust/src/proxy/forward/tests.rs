use super::*;
fn parts_for(uri: &str) -> Parts {
    Request::builder().uri(uri).body(()).unwrap().into_parts().0
}

fn add_test_marker(mut value: serde_json::Value, original_size: usize) -> (Vec<u8>, usize, usize) {
    value["lean_ctx_touched"] = serde_json::Value::Bool(true);
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
        "http://0.0.0.0:4000",
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
    ] {
        assert!(
            !crate::proxy::codec::upstream_is_local(remote),
            "{remote} must not be local"
        );
    }
}

// --- enterprise#51: fail-open single retry ---

#[test]
fn retry_covers_exactly_not_processed_statuses() {
    // Retryable: the upstream explicitly did not process the request.
    for code in [429_u16, 502, 503] {
        assert!(
            is_retryable_status(reqwest::StatusCode::from_u16(code).unwrap()),
            "{code} must be retryable"
        );
    }
    // Not retryable: success, client errors, and "may have processed".
    for code in [200_u16, 400, 401, 404, 500, 504] {
        assert!(
            !is_retryable_status(reqwest::StatusCode::from_u16(code).unwrap()),
            "{code} must NOT be retryable"
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
    assert!(should_forward_request_header("content-encoding", true));
    assert!(!should_forward_request_header("content-encoding", false));

    let decoded = zstd::decode_all(prepared.body.as_slice()).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(parsed["lean_ctx_touched"], true);
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
    assert_eq!(parsed["lean_ctx_touched"], true);
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

#[test]
fn forwards_openai_project_and_auth_headers() {
    // #366: project-scoped OpenAI keys carry the scope via `OpenAI-Project`.
    // It must be forwarded upstream, otherwise the Responses API rejects the
    // call with `Missing scopes: api.responses.write`.
    for required in ["authorization", "openai-project", "openai-organization"] {
        assert!(
            ALLOWED_REQUEST_HEADERS.contains(&required),
            "request header `{required}` must be forwarded upstream"
        );
    }
}

#[test]
fn forwards_chatgpt_codex_oauth_headers() {
    for required in [
        "authorization",
        "chatgpt-account-id",
        "x-openai-fedramp",
        "x-openai-internal-codex-residency",
        "x-openai-product-sku",
        "oai-product-sku",
        "x-client-request-id",
        "x-codex-installation-id",
        "x-codex-turn-metadata",
        "x-openai-subagent",
        "x-codex-turn-state",
        "originator",
    ] {
        assert!(
            is_allowed_request_header(required),
            "request header `{required}` must be forwarded upstream"
        );
    }
}

#[test]
fn forwards_streamable_http_mcp_headers() {
    for required in ["mcp-session-id", "last-event-id"] {
        assert!(
            ALLOWED_REQUEST_HEADERS.contains(&required),
            "request header `{required}` must be forwarded upstream"
        );
    }
    assert!(
        is_forwarded_response_header("mcp-session-id"),
        "MCP session id response header must be forwarded downstream"
    );
}

#[test]
fn forwards_codex_state_response_headers() {
    for required in [
        "x-codex-turn-state",
        "x-codex-primary-used-percent",
        "openai-model",
        "x-models-etag",
        "x-reasoning-included",
        "x-oai-request-id",
        "cf-ray",
        "x-openai-authorization-error",
        "x-error-json",
    ] {
        assert!(
            is_forwarded_response_header(required),
            "response header `{required}` must be forwarded downstream"
        );
    }
}

#[test]
fn internal_project_tag_is_not_forwarded() {
    assert!(!is_allowed_request_header("x-leanctx-project"));
}
