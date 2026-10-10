//! Upstream HTTP send and client-facing response construction.

use axum::{
    body::Body,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};

use super::headers::{
    connection_names, is_forwarded_response_header, should_forward_request_header,
};
use crate::proxy::ProxyState;
use crate::proxy::usage::{Provider, Scanner, WireContext, tee_stream};

/// Sends the request upstream once: the client's SDK retries as it sees fit,
/// honouring the upstream's `retry-after`. When no answer comes, the error
/// becomes the client's answer through [`unreachable_response`].
pub(crate) async fn send_upstream(
    state: &ProxyState,
    parts: &Parts,
    url: &str,
    body: Vec<u8>,
    provider_label: &str,
    preserve_content_encoding: bool,
) -> Result<reqwest::Response, reqwest::Error> {
    let hop_by_hop = connection_names(&parts.headers);
    let mut req = state.client.request(parts.method.clone(), url);
    for (key, value) in &parts.headers {
        if should_forward_request_header(key.as_str(), preserve_content_encoding, &hop_by_hop) {
            req = req.header(key.clone(), value.clone());
        }
    }
    req.body(body)
        .send()
        .await
        .inspect_err(|e| tracing::error!("sobre proxy: {provider_label} upstream error: {e}"))
}

/// The 502 for a request no answer came back to, in the error shape both
/// providers' clients read (`error.type`, `error.message`), so the agent
/// can tell the failure is the proxy's and not the provider's.
pub(crate) fn unreachable_response(url: &str, error: &reqwest::Error) -> Response {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    // The innermost cause says what happened ("Connection refused"); the
    // outer ones only repeat the URL.
    let mut cause: &dyn std::error::Error = error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    let what = if error.is_connect() {
        format!("upstream {host} unreachable")
    } else if error.is_timeout() {
        format!("upstream {host} did not answer in time")
    } else {
        format!("request to upstream {host} failed")
    };
    let body = serde_json::json!({
        "type": "error",
        "error": {
            "type": "proxy_error",
            "message": format!("sobre proxy: {what}: {cause}"),
        },
    });
    (StatusCode::BAD_GATEWAY, axum::Json(body)).into_response()
}

pub(crate) async fn build_response(
    response: reqwest::Response,
    provider: Provider,
    wire: WireContext,
) -> Result<Response, StatusCode> {
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::OK);
    let resp_headers = response.headers().clone();
    let hop_by_hop = connection_names(&resp_headers);
    let mut scanner = Scanner::new(provider).with_wire_context(Some(Box::new(wire)));

    let mut resp = Response::builder().status(status);
    for (k, v) in &resp_headers {
        if is_forwarded_response_header(k.as_str(), &hop_by_hop) {
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
