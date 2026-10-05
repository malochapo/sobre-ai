//! Universal provider routes — the request-path half of the
//! universal-provider-framework (enterprise#7).
//!
//! `/providers/{id}/...` forwards to the provider with that id in the closed
//! list of `core::config::proxy`, speaking its [`WireShape`].
//! Only the URL is configuration (`[proxy.upstreams]`).
//!
//! Shape ≠ identity: the proxy understands four wire dialects (the shapes) and
//! several provider identities map onto them. Compression, introspection and
//! usage metering all run exactly as they do for the built-in routes of the
//! same shape. The caller's own credentials are forwarded verbatim, except for
//! Bedrock, which the proxy signs with SigV4.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    response::Response,
};

use super::{ProxyState, forward};
use crate::core::config::WireShape;

/// Request extension carrying the registry identity of the serving provider.
///
/// Shape ≠ identity (module docs): the forward path only knows the wire shape
/// ("OpenAI"), but usage metering must attribute to the provider *identity*
/// ("foundry", "local") — otherwise every OpenAI-shaped registry entry shows
/// up as "OpenAI" in `usage_events` and the admin breakdown (enterprise#20).
/// `local` carries the entry's resolved local-inference flag so shadow-rate
/// billing works for non-loopback local endpoints too (host.docker.internal).
#[derive(Debug, Clone)]
pub(super) struct RegistryProviderId {
    pub id: String,
    pub local: bool,
}

/// Registry ids that share the Grok / xAI dual-rail (subscription `grok-chat`,
/// API-key `xai`). OpenAI wire shape; separate `proxy status` bucket.
pub(super) fn is_grok_provider_id(id: &str) -> bool {
    matches!(
        id.trim().to_ascii_lowercase().as_str(),
        "grok-chat" | "xai" | "grok"
    )
}

/// Registry ids that share the Command Code gateway rail (`commandcode`).
/// Separate `proxy status` bucket so traffic does not fold into the wire-shape
/// line.
pub(super) fn is_commandcode_provider_id(id: &str) -> bool {
    matches!(
        id.trim().to_ascii_lowercase().as_str(),
        "commandcode" | "command-code"
    )
}

/// Per-upstream stats label for a registry route.
///
/// Wire shape stays `OpenAI`/`Anthropic`/… for compression; identity for
/// `ProxyStats` may differ (Grok must not fold into the OpenAI line).
pub(super) fn stats_label<'a>(registry_id: Option<&str>, shape_label: &'a str) -> &'a str {
    match registry_id {
        Some(id) if is_grok_provider_id(id) => "Grok",
        Some(id) if is_commandcode_provider_id(id) => "CommandCode",
        _ => shape_label,
    }
}

/// True when an OpenAI-shaped registry request should use the Responses
/// compressor (`input` / `function_call_output`) rather than Chat Completions.
pub(super) fn is_openai_responses_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path == "/responses"
        || path == "/v1/responses"
        || path.starts_with("/responses/")
        || path.starts_with("/v1/responses/")
}

pub async fn handler(
    State(state): State<ProxyState>,
    Path((id, rest)): Path<(String, String)>,
    mut req: Request<Body>,
) -> Result<Response, StatusCode> {
    let Some(provider) = state.upstream_snapshot().provider_by_id(&id).cloned() else {
        tracing::warn!("lean-ctx proxy: unknown registry provider '{id}' (404)");
        return Err(StatusCode::NOT_FOUND);
    };
    req.extensions_mut().insert(RegistryProviderId {
        id: provider.id.clone(),
        local: provider.local,
    });

    // Strip the `/providers/{id}` prefix so the upstream sees the bare provider
    // path: `/providers/foundry/v1/chat/completions` → `/v1/chat/completions`.
    let path = format!("/{rest}");
    let uri = match req.uri().query() {
        Some(q) => format!("{path}?{q}").parse::<axum::http::Uri>(),
        None => path.parse::<axum::http::Uri>(),
    }
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    *req.uri_mut() = uri;

    if provider.shape == WireShape::Bedrock {
        super::bedrock::validate_invoke_request(&req)?;
        super::bedrock::attach_signing_context(&provider, &mut req)?;
    }

    match provider.shape {
        WireShape::Anthropic => {
            forward::forward_request(
                State(state),
                req,
                &provider.base_url,
                "/v1/messages",
                super::bedrock::passthrough_request_body,
                "Anthropic",
                &[],
            )
            .await
        }
        WireShape::OpenAi => {
            // Built-in OpenAI routes pick Chat Completions vs Responses by path
            // (`/v1/chat/completions` vs `/v1/responses`). Registry providers
            // must do the same: Grok CLI hits `/providers/grok-chat/v1/responses`
            // with `function_call_output` in `input`, which the Chat compressor
            // ignores (it only rewrites `messages`). Path already has the
            // `/providers/{id}` prefix stripped above.
            if is_openai_responses_path(req.uri().path()) {
                forward::forward_request(
                    State(state),
                    req,
                    &provider.base_url,
                    "/v1/responses",
                    super::openai_responses::compress_request_body,
                    "OpenAI",
                    &[],
                )
                .await
            } else {
                forward::forward_request(
                    State(state),
                    req,
                    &provider.base_url,
                    "/v1/chat/completions",
                    super::openai::compress_request_body,
                    "OpenAI",
                    &[],
                )
                .await
            }
        }
        WireShape::Gemini => {
            // Gemini carries the model in the URL path, not the body (#840).
            let model = super::usage::gemini_model_from_path(req.uri().path());
            forward::forward_request(
                State(state),
                req,
                &provider.base_url,
                "/",
                move |body, size| {
                    super::google::compress_request_body(body, size, model.as_deref())
                },
                "Gemini",
                &["application/x-ndjson"],
            )
            .await
        }
        WireShape::Bedrock => {
            forward::forward_request(
                State(state),
                req,
                &provider.base_url,
                "/",
                super::bedrock::passthrough_request_body,
                "Bedrock",
                &["application/vnd.amazon.eventstream"],
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_label_maps_grok_rails_to_grok_bucket() {
        assert_eq!(stats_label(Some("grok-chat"), "OpenAI"), "Grok");
        assert_eq!(stats_label(Some("xai"), "OpenAI"), "Grok");
        assert_eq!(stats_label(Some("GROK"), "OpenAI"), "Grok");
        assert_eq!(stats_label(Some("foundry"), "OpenAI"), "OpenAI");
        assert_eq!(stats_label(None, "OpenAI"), "OpenAI");
        assert_eq!(stats_label(Some("local"), "OpenAI"), "OpenAI");
    }

    #[test]
    fn is_grok_provider_id_accepts_dual_rail_ids() {
        assert!(is_grok_provider_id("grok-chat"));
        assert!(is_grok_provider_id("xai"));
        assert!(is_grok_provider_id(" Grok "));
        assert!(!is_grok_provider_id("openai"));
        assert!(!is_grok_provider_id("foundry"));
    }

    #[test]
    fn stats_label_maps_commandcode_rail_to_commandcode_bucket() {
        assert_eq!(stats_label(Some("commandcode"), "OpenAI"), "CommandCode");
        assert_eq!(stats_label(Some("command-code"), "OpenAI"), "CommandCode");
        assert_eq!(stats_label(Some(" CommandCode "), "OpenAI"), "CommandCode");
    }

    #[test]
    fn is_commandcode_provider_id_accepts_rail_ids() {
        assert!(is_commandcode_provider_id("commandcode"));
        assert!(is_commandcode_provider_id("command-code"));
        assert!(is_commandcode_provider_id(" COMMANDCODE "));
        assert!(!is_commandcode_provider_id("openai"));
        assert!(!is_commandcode_provider_id("grok"));
    }

    #[test]
    fn is_openai_responses_path_detects_responses_api() {
        assert!(is_openai_responses_path("/v1/responses"));
        assert!(is_openai_responses_path("/v1/responses/"));
        assert!(is_openai_responses_path("/responses"));
        assert!(is_openai_responses_path(
            "/v1/responses/resp_123/input_items"
        ));
        assert!(!is_openai_responses_path("/v1/chat/completions"));
        assert!(!is_openai_responses_path("/v1/models"));
        assert!(!is_openai_responses_path("/v1/responsesx"));
    }
}
