//! End-to-end smoke test for the LLM proxy runtime path: bind → serve → health
//! → auth guard → shutdown. This exercises the *actual* runtime (a real TCP
//! listener and the full middleware stack), not just unit-level helpers.
//!
//! It stays hermetic and offline: a provider route sent without credentials is
//! rejected by the auth guard (401) *before* any upstream is contacted, so no
//! network egress and no real provider key are required.

use std::time::Duration;

/// Reserve an ephemeral loopback port, then release it for the proxy to bind.
/// The brief race between release and re-bind is tolerated by the health poll.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe socket");
    let port = listener.local_addr().expect("probe local_addr").port();
    drop(listener);
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_serves_health_and_enforces_auth() {
    let port = free_port();
    let token = "smoke-token".to_string();

    let server = tokio::spawn({
        let token = token.clone();
        async move {
            let _ = sobre::proxy::start_proxy_with_token(port, token).await;
        }
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // Wait for the proxy to come up (bind + serve), up to ~5s.
    let mut healthy = false;
    for _ in 0..50 {
        if let Ok(resp) = client.get(format!("{base}/health")).send().await
            && resp.status().is_success()
        {
            healthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(healthy, "proxy /health never became ready");

    // A provider route with no credentials must be rejected by the auth guard
    // (401) — this happens before any upstream forwarding, keeping the test
    // offline while still proving routing + host guard + auth guard are wired.
    let unauth = client
        .post(format!("{base}/v1/messages"))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request to proxy");
    assert_eq!(
        unauth.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "provider route without a token/key must return 401"
    );

    // Codex opens the Responses API as a WebSocket first. The refusal must be
    // 426 on both spellings, with or without a token, and name the protocol to
    // use: anything else costs retries or fails the turn.
    for path in ["/v1/responses", "/responses"] {
        for with_token in [true, false] {
            let mut req = client
                .get(format!("{base}{path}"))
                .header("connection", "Upgrade")
                .header("upgrade", "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
            if with_token {
                req = req.bearer_auth(&token);
            }
            let ws = req.send().await.expect("request to proxy");
            assert_eq!(
                ws.status(),
                reqwest::StatusCode::UPGRADE_REQUIRED,
                "GET {path} (token: {with_token}) must answer 426"
            );
            assert_eq!(
                ws.headers().get("upgrade").and_then(|v| v.to_str().ok()),
                Some("HTTP/1.1"),
                "a 426 names the protocol to use"
            );
        }
    }

    // Gemini is not served.
    let gemini = client
        .post(format!(
            "{base}/v1beta/models/gemini-2.5-pro:generateContent"
        ))
        .bearer_auth(&token)
        .body("{}")
        .send()
        .await
        .expect("request to proxy");
    assert_eq!(gemini.status(), reqwest::StatusCode::NOT_FOUND);

    // Teardown: cancel the serve task (drops the listener).
    server.abort();
}
