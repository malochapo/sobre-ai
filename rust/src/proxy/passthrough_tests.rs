//! What the proxy passes through as it came: headers both ways, upstream
//! errors with their status, headers and body, one request per request, and
//! an answer that says the proxy failed when the upstream cannot be reached.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// Runs the proxy in front of `anthropic` and returns its base URL.
async fn proxy_in_front_of(anthropic: String) -> String {
    let upstreams = Upstreams {
        anthropic,
        openai: "http://127.0.0.1:9".into(),
    };
    let (_, rx) = tokio::sync::watch::channel(Arc::new(upstreams));
    let state = ProxyState {
        client: reqwest::Client::new(),
        port: 0,
        upstreams: rx,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app(state, "proxy-token".into())).await });
    base
}

/// A loopback upstream that counts its requests and answers each with a 429
/// carrying the headers a provider sends, and the request headers it
/// received in the body.
async fn rate_limited_upstream(hits: Arc<AtomicUsize>) -> String {
    let upstream = Router::new().fallback(move |req: Request<Body>| {
        let hits = hits.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let received: Vec<String> = req
                .headers()
                .keys()
                .map(|name| name.as_str().to_string())
                .collect();
            Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("content-type", "application/json")
                .header("request-id", "req_123")
                .header("x-should-retry", "false")
                .header("retry-after", "7")
                .header("retry-after-ms", "7000")
                .header("anthropic-ratelimit-unified-status", "rejected")
                .header("x-made-up-provider-header", "kept")
                .body(Body::from(
                    serde_json::json!({ "received": received }).to_string(),
                ))
                .unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await });
    format!("http://{addr}")
}

#[tokio::test]
async fn headers_and_errors_pass_through_both_ways_once() {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = proxy_in_front_of(rate_limited_upstream(hits.clone()).await).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "fake-key")
        .header("content-type", "application/json")
        .header("x-stainless-retry-count", "0")
        .header("x-app", "cli")
        .header("x-claude-code-session-id", "s1")
        .header("accept-encoding", "gzip")
        .body(r#"{"model":"m","messages":[]}"#)
        .send()
        .await
        .unwrap();

    // The upstream's answer comes back as it was sent.
    assert_eq!(resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    for (name, value) in [
        ("request-id", "req_123"),
        ("x-should-retry", "false"),
        ("retry-after", "7"),
        ("retry-after-ms", "7000"),
        ("anthropic-ratelimit-unified-status", "rejected"),
        ("x-made-up-provider-header", "kept"),
    ] {
        assert_eq!(
            resp.headers().get(name).and_then(|v| v.to_str().ok()),
            Some(value),
            "response header {name}"
        );
    }
    let body: serde_json::Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    let received: Vec<&str> = body["received"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    for name in [
        "x-api-key",
        "x-stainless-retry-count",
        "x-app",
        "x-claude-code-session-id",
    ] {
        assert!(
            received.contains(&name),
            "{name} not forwarded: {received:?}"
        );
    }
    // The proxy reads the usage in the answer: it asks for it uncompressed.
    assert!(!received.contains(&"accept-encoding"), "{received:?}");

    // The client retries as it sees fit: the proxy sent the request once.
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_unreachable_upstream_gets_an_answer_from_the_proxy() {
    // Nothing listens on a port just released.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let base = proxy_in_front_of(format!("http://127.0.0.1:{closed}")).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "fake-key")
        .body(r#"{"model":"m","messages":[]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!(body["error"]["type"], "proxy_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("sobre proxy: upstream 127.0.0.1 unreachable: "),
        "{message}"
    );
}
