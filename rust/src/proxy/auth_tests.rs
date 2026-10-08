//! Split from `proxy/mod.rs` (#660 LOC gate): `auth_tests`.

use super::*;

// P0-4 (#416): the proxy must never run unauthenticated — `None` means
// "resolve the session token", not "no auth".
#[test]
fn effective_auth_token_never_yields_empty() {
    let _env = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    crate::test_env::set_var("SOBRE_DATA_DIR", tmp.path());

    assert_eq!(effective_auth_token(Some("tok".into())), "tok");
    let auto = effective_auth_token(None);
    assert!(!auto.trim().is_empty(), "None must auto-resolve a token");
    let blank = effective_auth_token(Some("   ".into()));
    assert!(!blank.trim().is_empty(), "blank tokens must be replaced");

    crate::test_env::remove_var("SOBRE_DATA_DIR");
}

#[test]
fn is_provider_route_v1() {
    assert!(is_provider_route("/v1/chat/completions"));
    assert!(is_provider_route("/v1/messages"));
    assert!(is_provider_route("/v1/completions"));
}

#[test]
fn is_provider_route_anthropic_subpaths() {
    assert!(is_provider_route("/v1/messages/count_tokens"));
    assert!(is_provider_route("/v1/messages/batches"));
    assert!(is_provider_route("/v1/messages/batches/batch_123"));
}

#[test]
fn is_provider_route_chat() {
    assert!(is_provider_route("/chat/completions"));
}

#[test]
fn is_provider_route_rejects_non_provider() {
    assert!(!is_provider_route("/health"));
    assert!(!is_provider_route("/api/v2/test"));
    assert!(!is_provider_route("/"));
    assert!(!is_provider_route(
        "/v1beta/models/gemini-2.5-pro:generateContent"
    ));
    assert!(!is_provider_route("/providers/ollama/v1/chat/completions"));
}

#[test]
fn is_provider_route_model_list() {
    // `GET /v1/models` (and the bare `/models` of clients whose base URL
    // omits `/v1`) must authenticate like any provider route.
    assert!(is_provider_route("/v1/models"));
    assert!(is_provider_route("/models"));
    assert!(is_provider_route("/models/gpt-5.5"));
    // Nothing else under a bare `/models*` prefix becomes a provider route.
    assert!(!is_provider_route("/modelsx"));
}

fn build_request(headers: &[(&str, &str)], path: &str) -> axum::extract::Request {
    let mut builder = axum::http::Request::builder().uri(path);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    builder.body(axum::body::Body::empty()).unwrap()
}

#[test]
fn has_provider_api_key_x_api_key() {
    let req = build_request(&[("x-api-key", "sk-ant-abc123")], "/v1/messages");
    assert!(has_provider_api_key(&req));
}

#[test]
fn has_provider_api_key_azure() {
    let req = build_request(&[("api-key", "deadbeef")], "/v1/completions");
    assert!(has_provider_api_key(&req));
}

#[test]
fn has_provider_api_key_bearer_sk() {
    let req = build_request(
        &[("authorization", "Bearer [REDACTED:Bearer token]")],
        "/v1/chat/completions",
    );
    assert!(has_provider_api_key(&req));
}

#[test]
fn has_provider_api_key_empty_rejected() {
    let req = build_request(&[("x-api-key", "  ")], "/v1/messages");
    assert!(!has_provider_api_key(&req));
}

#[test]
fn has_provider_api_key_no_headers() {
    let req = build_request(&[], "/v1/messages");
    assert!(!has_provider_api_key(&req));
}

#[test]
fn has_provider_api_key_accepts_non_sk_bearer() {
    // #362: OpenAI-*compatible* providers (Azure, OpenRouter, Groq, vLLM/
    // Ollama gateways, project/service keys) issue keys without the sk-/gsk_
    // prefix. OpenCode (@ai-sdk/openai) forwards them as `Bearer <key>`; they
    // must authenticate on a loopback provider route. The upstream validates
    // the real key — the proxy never injects one.
    for key in [
        "Bearer [REDACTED:Bearer token]", // OpenRouter
        "Bearer [REDACTED:Bearer token]", // (still works)
        "Bearer [REDACTED:Bearer token]", // gateway/service token
        "Bearer [REDACTED:Bearer token]", // opaque
    ] {
        let req = build_request(&[("authorization", key)], "/v1/responses");
        assert!(
            has_provider_api_key(&req),
            "non-sk Bearer must count as a provider credential: {key}"
        );
    }
}

#[test]
fn has_provider_api_key_empty_bearer_rejected() {
    // A blank credential — or a bare scheme word with no token (some HTTP
    // stacks trim trailing whitespace down to just "Bearer") — is not auth.
    for bad in ["Bearer    ", "", "Bearer", "bearer", "   "] {
        let req = build_request(&[("authorization", bad)], "/responses");
        assert!(
            !has_provider_api_key(&req),
            "blank/scheme-only Authorization must not authenticate: {bad:?}"
        );
    }
}

#[test]
fn auth_error_response_provider_hint() {
    let resp = auth_error_response("/v1/messages");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn auth_error_response_generic_hint() {
    let resp = auth_error_response("/status");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn x_sobre_project_never_forwarded_upstream() {
    // Internal gateway header: it must NOT be on the upstream allowlist,
    // otherwise org-internal project names leak to the provider.
    assert!(!forward::is_allowed_request_header("x-sobre-project"));
}

// --- #353: bare provider endpoints (OpenCode / @ai-sdk/openai) ---

#[test]
fn is_provider_route_bare_responses_and_messages() {
    // Clients that point their base URL at the proxy root (no `/v1`) send the
    // bare endpoint; auth must still recognise it as a provider route.
    assert!(is_provider_route("/responses"));
    assert!(is_provider_route("/responses/resp_123/input_items"));
    assert!(is_provider_route("/messages"));
}

#[test]
fn canonical_provider_path_rewrites_bare_endpoints() {
    assert_eq!(
        canonical_provider_path("/responses").as_deref(),
        Some("/v1/responses")
    );
    assert_eq!(
        canonical_provider_path("/chat/completions").as_deref(),
        Some("/v1/chat/completions")
    );
    assert_eq!(
        canonical_provider_path("/messages").as_deref(),
        Some("/v1/messages")
    );
    assert_eq!(
        canonical_provider_path("/models/gpt-5.5").as_deref(),
        Some("/v1/models/gpt-5.5")
    );
}

#[test]
fn canonical_provider_path_preserves_subpaths() {
    assert_eq!(
        canonical_provider_path("/responses/resp_abc/cancel").as_deref(),
        Some("/v1/responses/resp_abc/cancel")
    );
    assert_eq!(
        canonical_provider_path("/messages/batches/batch_1").as_deref(),
        Some("/v1/messages/batches/batch_1")
    );
}

#[test]
fn canonical_provider_path_ignores_already_canonical_and_unknown() {
    // Already canonical → no rewrite (avoids `/v1/v1/...`).
    assert_eq!(canonical_provider_path("/v1/responses"), None);
    assert_eq!(canonical_provider_path("/v1/chat/completions"), None);
    // Unrelated paths are untouched.
    assert_eq!(canonical_provider_path("/health"), None);
    assert_eq!(canonical_provider_path("/responsesx"), None);
    assert_eq!(canonical_provider_path("/"), None);
}

#[test]
fn canonical_provider_path_collapses_double_v1_prefix() {
    // The OpenAI base URL sobre writes carries `/v1` (#366); a client treating
    // it as an origin and appending `/v1/...` itself produces a double prefix.
    assert_eq!(
        canonical_provider_path("/v1/v1/responses").as_deref(),
        Some("/v1/responses")
    );
    assert_eq!(
        canonical_provider_path("/v1/v1/chat/completions").as_deref(),
        Some("/v1/chat/completions")
    );
}

#[test]
fn normalized_provider_uri_rewrites_path_and_preserves_query() {
    use axum::http::Uri;
    let uri: Uri = "/responses?stream=true".parse().unwrap();
    let rewritten = normalized_provider_uri(&uri).expect("bare /responses must rewrite");
    assert_eq!(rewritten.path(), "/v1/responses");
    assert_eq!(rewritten.query(), Some("stream=true"));
    assert_eq!(
        rewritten
            .path_and_query()
            .map(axum::http::uri::PathAndQuery::as_str),
        Some("/v1/responses?stream=true")
    );
}

#[test]
fn normalized_provider_uri_noop_for_canonical() {
    use axum::http::Uri;
    let uri: Uri = "/v1/responses".parse().unwrap();
    assert!(normalized_provider_uri(&uri).is_none());
}

// --- DNS-rebinding guard ---

#[test]
fn host_allowed_passes_only_loopback() {
    for h in [
        "127.0.0.1",
        "127.0.0.1:4444",
        "localhost:9999",
        "[::1]:80",
        "LOCALHOST.",
    ] {
        assert!(host_allowed(h), "loopback host must pass: {h}");
    }
    for h in [
        "gateway.example.com",
        "evil.example.com",
        "127.0.0.1.evil.io",
        "0.0.0.0",
    ] {
        assert!(!host_allowed(h), "non-loopback host must be refused: {h}");
    }
}
