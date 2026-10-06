//! Shared upstream forward path for OpenAI-compatible providers.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};

use super::ProxyState;

#[cfg(feature = "shape-xlat")]
mod xlat;

mod headers;
mod prepare;
mod trace_id;
mod transport;

#[cfg(test)]
mod tests;

#[allow(unused_imports)] // re-exported for proxy::* and tests
pub(super) use headers::{
    ALLOWED_REQUEST_HEADERS, FORWARDED_HEADERS, is_allowed_request_header,
    is_forwarded_response_header,
};
pub(super) use transport::xlat_stream_body;

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
pub(super) use prepare::{prepare_request_body, wire_context};

const HEADROOM_COMPRESSED_HEADER: &str = "x-headroom-compressed";

/// Check whether an incoming request was already compressed by Headroom.
pub(super) fn is_headroom_compressed(parts: &axum::http::request::Parts) -> bool {
    parts
        .headers
        .get(HEADROOM_COMPRESSED_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

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
    provider_label: &str,
    extra_stream_types: &[&str],
) -> Result<Response, StatusCode> {
    let (mut parts, body) = req.into_parts();
    let trace_id = trace_id::extract_or_generate_trace_id(&parts.headers);
    let body_limit = super::bedrock::request_body_limit(&parts).unwrap_or_else(max_body_bytes);
    let body_bytes = axum::body::to_bytes(body, body_limit)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;

    // Active router (enterprise#13): may rewrite `model` in the parsed body
    // (before compression, so exactly one serialization) and re-target the
    // upstream within the same wire shape. Fail-open: any miss routes nothing.
    let routing_rules = crate::core::config::Config::load().proxy.routing.clone();
    let route_upstreams = routing_rules.is_active().then(|| state.upstream_snapshot());
    // Cross-shape translation (enterprise#16) only exists for the exact
    // messages-create call — count_tokens/batches subpaths have no OpenAI
    // equivalent and must stay within-shape.
    let xlat_ok = cfg!(feature = "shape-xlat")
        && provider_label == "Anthropic"
        && parts
            .uri
            .path()
            .trim_end_matches('/')
            .ends_with("/v1/messages");
    let route_hook = |parsed: &mut serde_json::Value| {
        route_upstreams.as_ref().and_then(|up| {
            super::routing::route_request(parsed, provider_label, up, &routing_rules, xlat_ok)
        })
    };
    if is_headroom_compressed(&parts) {
        super::prefix_cache_stats::record_headroom_compat();
    }
    let prepared = prepare::prepare_request_body(
        &parts,
        &body_bytes,
        compress_body,
        route_hook,
        upstream_base,
        provider_label == "OpenAI",
    )?;
    let original_size = prepared.original_size;
    let compressed_size = prepared.compressed_size;
    let compression_candidate = prepared.compression_candidate;
    let preserve_content_encoding = prepared.preserve_content_encoding;
    let route = prepared.route;
    let parsed = prepared.parsed;
    // Apply the routing decision to the wire: re-target the upstream.
    let upstream_base = route
        .as_ref()
        .and_then(|r| r.upstream_base.as_deref())
        .unwrap_or(upstream_base);
    if let Some(ref parsed) = parsed {
        let provider = match provider_label {
            "Anthropic" | "Bedrock" => super::introspect::Provider::Anthropic,
            "OpenAI" => super::introspect::Provider::OpenAi,
            _ => super::introspect::Provider::Gemini,
        };
        let breakdown = super::introspect::analyze_request(parsed, provider);
        state.introspect.record(breakdown);
    }
    if compression_candidate {
        state
            .stats
            .record_provider_request(provider_label, original_size, compressed_size);
    }

    let tokens_saved = original_size.saturating_sub(compressed_size) as u64 / 4;
    super::metrics::record_request(tokens_saved, compressed_size as u64);

    let model = parsed
        .as_ref()
        .and_then(|v| v.get("model"))
        .and_then(|m| m.as_str());
    super::cost::record(
        model,
        tokens_saved,
        original_size as u64,
        compressed_size as u64,
    );

    // Cross-shape route (enterprise#16): the body now speaks OpenAI Chat
    // Completions — address the matching endpoint instead of the caller's
    // `/v1/messages` path, and scan the response with the OpenAI parser.
    let xlat = route.as_ref().is_some_and(|r| r.xlat);
    let upstream_url = if xlat {
        format!("{upstream_base}/v1/chat/completions")
    } else {
        crate::proxy::codec::build_upstream_url(&parts, upstream_base, default_path)
    };

    let forwarded_body = super::bedrock::finalize_request(
        provider_label,
        &mut parts,
        &body_bytes,
        prepared.body,
        body_limit,
        &upstream_url,
    )?;

    if let Some(ref pre) = parsed {
        {
            let system_val = pre.get("system");
            if let Some(msgs) = pre.get("messages").and_then(|m| m.as_array()) {
                let conv_id = super::prefix_replay::conversation_id(system_val, msgs);
                super::prefix_replay::record_forwarded(
                    conv_id,
                    forwarded_body.clone(),
                    msgs,
                    msgs.len(),
                );
            }
        }
    }

    let response = transport::send_upstream(
        &state,
        &parts,
        &upstream_url,
        forwarded_body,
        provider_label,
        preserve_content_encoding,
    )
    .await?;

    // Measured usage: read the real model + billed tokens from the response.
    // Gemini puts the model in the URL path, not the request/response body.
    // Translated requests get OpenAI-shape responses regardless of the label.
    let usage_provider = if xlat {
        super::usage::Provider::OpenAi
    } else {
        super::usage::Provider::from_label(provider_label)
    };
    let url_model = if usage_provider == super::usage::Provider::Gemini {
        super::usage::gemini_model_from_path(parts.uri.path())
    } else {
        None
    };

    // Request-side context: the project tag from the auth guard, wire savings
    // and baseline inputs, stamped onto the usage record.
    // A routed request is attributed to the provider actually serving it, and
    // carries the originally requested model as routed_from (enterprise#13).
    let mut wire = prepare::wire_context(
        &parts,
        provider_label,
        upstream_base,
        tokens_saved,
        original_size,
    );
    if let Some(route) = &route {
        wire.routed_from = Some(route.routed_from.clone());
        if let Some(id) = &route.provider_id {
            wire.provider = id.clone();
        }
        // Registry route targets carry their own local-inference flag
        // (shadow-rate billing); built-in targets keep the URL heuristic.
        if let Some(local) = route.local {
            wire.is_local = local;
        }
    }
    let wire = Some(wire);
    let mut response = transport::build_response(
        response,
        extra_stream_types,
        usage_provider,
        url_model,
        wire,
        xlat,
    )
    .await?;
    trace_id::inject_trace_id(&mut response, &trace_id);
    Ok(response)
}
