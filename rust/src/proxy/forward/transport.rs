//! Upstream HTTP send and client-facing response construction.

use axum::{
    body::Body,
    http::{StatusCode, request::Parts},
    response::Response,
};

use super::headers::{is_forwarded_response_header, should_forward_request_header};
use crate::proxy::ProxyState;
use crate::proxy::codec::{is_retryable_status, retry_backoff};
use crate::proxy::usage::{Provider, Scanner, WireContext, tee_stream};

pub(crate) async fn send_upstream(
    state: &ProxyState,
    parts: &Parts,
    url: &str,
    body: Vec<u8>,
    provider_label: &str,
    preserve_content_encoding: bool,
) -> Result<reqwest::Response, StatusCode> {
    let send_once = |body: Vec<u8>| {
        let mut req = state.client.request(parts.method.clone(), url);
        for (key, value) in &parts.headers {
            let k = key.as_str().to_lowercase();
            if should_forward_request_header(&k, preserve_content_encoding) {
                req = req.header(key.clone(), value.clone());
            }
        }
        req.body(body).send()
    };

    // First attempt. The request body is fully buffered, and no response byte
    // has reached the client yet — retrying here is always safe for the
    // client connection; the status filter keeps it safe semantically.
    let first = send_once(body.clone()).await;
    let retry_reason = match &first {
        Ok(resp) if is_retryable_status(resp.status()) => {
            format!("status {}", resp.status().as_u16())
        }
        Err(e) if e.is_connect() || e.is_timeout() => format!("connect error: {e}"),
        Ok(resp) => {
            let _ = resp; // healthy (or non-retryable) response — pass through
            return first.map_err(|_| StatusCode::BAD_GATEWAY);
        }
        Err(e) => {
            tracing::error!("lean-ctx proxy: {provider_label} upstream error: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    tracing::warn!("lean-ctx proxy: {provider_label} upstream {retry_reason} — retrying once");
    retry_backoff().await;
    match send_once(body).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            // Second failure: surface the ORIGINAL outcome when it was an HTTP
            // response (its status/headers are more honest than our 502).
            tracing::error!("lean-ctx proxy: {provider_label} retry failed: {e}");
            first.map_err(|_| StatusCode::BAD_GATEWAY)
        }
    }
}

pub(crate) async fn build_response(
    response: reqwest::Response,
    provider: Provider,
    wire: WireContext,
) -> Result<Response, StatusCode> {
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::OK);
    let resp_headers = response.headers().clone();
    let mut scanner = Scanner::new(provider).with_wire_context(Some(Box::new(wire)));

    let mut resp = Response::builder().status(status);
    for (k, v) in &resp_headers {
        let ks = k.as_str().to_lowercase();
        if is_forwarded_response_header(&ks) {
            resp = resp.header(k, v);
        }
    }

    if response_is_sse(&resp_headers) {
        // Every chunk goes to the client unchanged while the scanner reads
        // the usage events on the way through.
        let body = Body::from_stream(tee_stream(Box::pin(response.bytes_stream()), scanner));
        return resp
            .body(body)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    }

    let resp_bytes = response
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    // Non-streaming: the whole body is one JSON object carrying `usage`.
    scanner.feed_body(&resp_bytes);
    if let Some(usage) = scanner.finalize() {
        crate::proxy::usage_meter::record(&usage);
    }

    resp.body(Body::from(resp_bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn response_is_sse(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
}
