//! The forward path every provider route shares: decode the body, compress
//! its tool results, re-encode, send upstream, and scan the response for the
//! usage the provider reports.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};

use super::ProxyState;
use super::usage::{Provider, WireContext};

mod headers;
mod prepare;
mod transport;

#[cfg(test)]
mod tests;

#[allow(unused_imports)] // re-exported for proxy::* and tests
pub(super) use headers::{
    ALLOWED_REQUEST_HEADERS, is_allowed_request_header, is_forwarded_response_header,
};

// Unit tests import these via `use super::*`.
#[cfg(test)]
#[allow(unused_imports)]
use super::codec::{
    RequestBodyEncoding, decode_gzip_bounded, encode_gzip, encode_zstd, is_retryable_status,
    request_body_encoding,
};
#[cfg(test)]
#[allow(unused_imports)]
use axum::http::request::Parts;
#[cfg(test)]
#[allow(unused_imports)]
use headers::should_forward_request_header;
#[cfg(test)]
#[allow(unused_imports)]
pub(super) use prepare::prepare_request_body;

/// Default request-body ceiling (MiB). A large-codebase refactor with several
/// big files in context easily exceeds the old 10 MiB cap, which surfaced to the
/// agent as a hard `400` mid-task, hence 64 MiB.
const DEFAULT_MAX_BODY_MB: usize = 64;

pub(super) fn max_body_bytes() -> usize {
    DEFAULT_MAX_BODY_MB.saturating_mul(1024 * 1024)
}

pub(crate) async fn forward_request(
    State(state): State<ProxyState>,
    req: Request<Body>,
    upstream_base: &str,
    default_path: &str,
    compress_body: impl FnOnce(serde_json::Value, usize) -> (Vec<u8>, usize, usize),
    provider: Provider,
) -> Result<Response, StatusCode> {
    let (parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, max_body_bytes())
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;

    let prepared = prepare::prepare_request_body(&parts, &body_bytes, compress_body)?;
    let upstream_url = super::codec::build_upstream_url(&parts, upstream_base, default_path);

    let response = transport::send_upstream(
        &state,
        &parts,
        &upstream_url,
        prepared.body,
        provider.label(),
        prepared.preserve_content_encoding,
    )
    .await?;

    let wire = WireContext {
        provider,
        request_model: prepared.request_model,
        is_local: super::codec::upstream_is_local(upstream_base),
    };
    transport::build_response(response, provider, wire).await
}
