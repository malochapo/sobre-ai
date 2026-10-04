//! Request-body preparation: parse, translate, compress.

use axum::http::{StatusCode, request::Parts};
use std::borrow::Cow;

use crate::proxy::codec::{
    RequestBodyEncoding, decode_gzip_bounded, decode_zstd_bounded, encode_gzip, encode_zstd,
    request_body_encoding,
};

use super::max_body_bytes;

#[cfg(feature = "shape-xlat")]
use super::xlat::translated_openai_body;

/// Builds the request-side [`WireContext`](crate::proxy::usage::WireContext) stamped
/// onto this turn's usage record: the project tag (inserted as a request
/// extension by the auth guard), the per-request compression
/// saving, the pre-compression token estimate (baseline input, enterprise#18)
/// and whether the serving upstream is local.
pub(crate) fn wire_context(
    parts: &Parts,
    provider_label: &str,
    upstream_base: &str,
    tokens_saved: u64,
    original_size: usize,
) -> Box<crate::proxy::usage::WireContext> {
    let tags = parts
        .extensions
        .get::<crate::proxy::request_tags::RequestTags>()
        .cloned()
        .unwrap_or_default();
    // Registry routes attribute usage to the provider identity ("foundry",
    // "local"), not the wire-shape label ("OpenAI") — shape ≠ identity. The
    // entry's resolved local flag rides along (shadow-rate billing for
    // non-loopback local endpoints, e.g. host.docker.internal).
    let registry = parts
        .extensions
        .get::<crate::proxy::providers::RegistryProviderId>();
    let provider = registry.map_or(provider_label, |r| r.id.as_str());
    let is_local = registry.map_or_else(
        || crate::proxy::codec::upstream_is_local(upstream_base),
        |r| r.local,
    );
    Box::new(crate::proxy::usage::WireContext {
        provider: provider.to_string(),
        project: tags.project,
        saved_tokens: tokens_saved,
        // bytes/4 — the same estimation basis the proxy stats use throughout.
        uncompressed_input_tokens: original_size as u64 / 4,
        is_local,
        routed_from: None,    // populated by the routing hook (wave 3)
        counterfactual: None, // populated after the probe spawn (#701)
    })
}

/// Output-savings arm (#895) for a request body, or `None` when no holdout is
/// active. Keyed per provider; OpenAI's Chat vs Responses bodies are
/// distinguished by the request path so each uses the matching cohort key.
pub(crate) fn cohort_arm(
    parsed: &serde_json::Value,
    provider_label: &str,
    default_path: &str,
) -> Option<crate::proxy::holdout::Arm> {
    let holdout = crate::core::config::Config::load()
        .proxy
        .output_holdout_fraction();
    if holdout <= 0.0 {
        return None;
    }
    let key = match provider_label {
        "Anthropic" => crate::proxy::holdout::anthropic_key(parsed),
        "OpenAI" => {
            if default_path.contains("responses") {
                crate::proxy::holdout::openai_responses_key(parsed)
            } else {
                crate::proxy::holdout::openai_chat_key(parsed)
            }
        }
        _ => crate::proxy::holdout::google_key(parsed),
    };
    Some(crate::proxy::holdout::assign(&key, holdout))
}

pub(crate) struct PreparedRequestBody {
    pub(crate) body: Vec<u8>,
    pub(crate) parsed: Option<serde_json::Value>,
    pub(crate) original_size: usize,
    pub(crate) compressed_size: usize,
    pub(crate) compression_candidate: bool,
    pub(crate) preserve_content_encoding: bool,
    /// Routing decision applied to the body (enterprise#13); `None` = passthrough.
    pub(crate) route: Option<crate::proxy::routing::RouteDecision>,
}

pub(crate) fn prepare_request_body(
    parts: &Parts,
    body_bytes: &[u8],
    compress_body: impl FnOnce(serde_json::Value, usize) -> (Vec<u8>, usize, usize),
    route_hook: impl FnOnce(&mut serde_json::Value) -> Option<crate::proxy::routing::RouteDecision>,
    default_upstream_base: &str,
    openai_shape: bool,
) -> Result<PreparedRequestBody, StatusCode> {
    let encoding = request_body_encoding(parts);
    let decoded = match encoding {
        RequestBodyEncoding::Identity => Cow::Borrowed(body_bytes),
        RequestBodyEncoding::Gzip => Cow::Owned(decode_gzip_bounded(body_bytes, max_body_bytes())?),
        RequestBodyEncoding::Zstd => Cow::Owned(decode_zstd_bounded(body_bytes, max_body_bytes())?),
        RequestBodyEncoding::Passthrough => {
            return Ok(PreparedRequestBody {
                body: body_bytes.to_vec(),
                parsed: None,
                original_size: body_bytes.len(),
                compressed_size: body_bytes.len(),
                compression_candidate: false,
                preserve_content_encoding: true,
                route: None,
            });
        }
    };

    let Some(mut parsed) = serde_json::from_slice::<serde_json::Value>(&decoded).ok() else {
        return Ok(PreparedRequestBody {
            body: body_bytes.to_vec(),
            parsed: None,
            original_size: body_bytes.len(),
            compressed_size: body_bytes.len(),
            compression_candidate: false,
            preserve_content_encoding: encoding != RequestBodyEncoding::Identity,
            route: None,
        });
    };

    // Router runs on the freshly parsed body, before compression: the model
    // swap lands in the same single serialization as the compression pass.
    let mut route = route_hook(&mut parsed);

    // Measured cost opt-in (#1179): when the *effective* upstream (post-
    // routing) is OpenRouter and the body speaks the OpenAI shape, ask for the
    // billed charge in the final usage payload. Other upstreams never see the
    // non-standard `usage` field (api.openai.com rejects unknown params).
    let effective_upstream = route
        .as_ref()
        .and_then(|r| r.upstream_base.as_deref())
        .unwrap_or(default_upstream_base);
    let wants_billed_cost =
        crate::proxy::usage_accounting::upstream_is_openrouter(effective_upstream)
            && crate::core::config::Config::load()
                .proxy
                .meters_openai_usage();
    let xlat_route = route.as_ref().is_some_and(|r| r.xlat);
    // `usage.include` is a Chat-Completions-only parameter: gate on the shape
    // AND the call path so a Responses-API body never carries it.
    let chat_completions_call = openai_shape
        && parts
            .uri
            .path()
            .trim_end_matches('/')
            .ends_with("/chat/completions");
    if wants_billed_cost && chat_completions_call && !xlat_route {
        crate::proxy::usage_accounting::inject_usage_include(&mut parsed);
    }

    let original_size = decoded.len();
    // Cross-shape route (enterprise#16): translate Messages→Chat-Completions
    // and compress with the target shape's compressor. An untranslatable body
    // fails open — the route is cancelled and the request forwards natively.
    let (logical_body, _, compressed_size) =
        if let Some(mut openai_body) = translated_openai_body(route.as_ref(), &parsed) {
            if wants_billed_cost {
                crate::proxy::usage_accounting::inject_usage_include(&mut openai_body);
            }
            crate::proxy::openai::compress_request_body(openai_body, original_size)
        } else {
            if route.as_ref().is_some_and(|r| r.xlat) {
                let decision = route.take().expect("checked is_some");
                tracing::warn!(
                    "lean-ctx proxy: request not translatable to OpenAI shape — \
                 cancelling route to '{}', forwarding natively",
                    decision.provider_id.as_deref().unwrap_or("?")
                );
                parsed["model"] = serde_json::Value::String(decision.routed_from);
            }
            compress_body(parsed.clone(), original_size)
        };
    let body = match encoding {
        RequestBodyEncoding::Identity => logical_body,
        RequestBodyEncoding::Gzip => encode_gzip(&logical_body)?,
        RequestBodyEncoding::Zstd => encode_zstd(&logical_body)?,
        RequestBodyEncoding::Passthrough => unreachable!("passthrough returned above"),
    };

    Ok(PreparedRequestBody {
        body,
        parsed: Some(parsed),
        original_size,
        compressed_size,
        compression_candidate: true,
        preserve_content_encoding: encoding != RequestBodyEncoding::Identity,
        route,
    })
}

#[cfg(not(feature = "shape-xlat"))]
pub(crate) fn translated_openai_body(
    _route: Option<&crate::proxy::routing::RouteDecision>,
    _parsed: &serde_json::Value,
) -> Option<serde_json::Value> {
    None
}
