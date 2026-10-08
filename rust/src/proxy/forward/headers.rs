//! Request/response header allowlists for the loopback proxy.

/// Request headers forwarded verbatim to the upstream provider. Anything not
/// listed here is stripped before the request leaves the loopback proxy.
///
/// `openai-project` (and `openai-organization`) must be forwarded: OpenCode and
/// the OpenAI SDK send the project scope via this header for project-scoped API
/// keys when calling the Responses API (`/responses`). Dropping it makes OpenAI
/// reject the request with `Missing scopes: api.responses.write` (#366).
pub(crate) const ALLOWED_REQUEST_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    // Azure OpenAI credential header, for an `openai` upstream on Azure.
    "api-key",
    "content-type",
    "accept",
    "user-agent",
    "originator",
    "anthropic-version",
    "anthropic-beta",
    "anthropic-dangerous-direct-browser-access",
    "openai-organization",
    "openai-project",
    "openai-beta",
    "chatgpt-account-id",
    "x-openai-fedramp",
    "x-openai-internal-codex-residency",
    "x-openai-internal-codex-responses-lite",
    "x-openai-product-sku",
    "oai-product-sku",
    "x-oai-attestation",
    "x-client-request-id",
    "x-codex-beta-features",
    "x-codex-installation-id",
    "x-codex-parent-thread-id",
    "x-openai-subagent",
    "x-codex-turn-state",
    "x-codex-turn-metadata",
    "x-codex-window-id",
    "x-openai-memgen-request",
    "x-responsesapi-include-timing-metrics",
    "mcp-session-id",
    "last-event-id",
    "cache-control",
    "x-session-id",
];

pub(crate) fn is_allowed_request_header(name: &str) -> bool {
    ALLOWED_REQUEST_HEADERS.contains(&name)
}

pub(crate) fn should_forward_request_header(name: &str, preserve_content_encoding: bool) -> bool {
    is_allowed_request_header(name)
        || (preserve_content_encoding && name.eq_ignore_ascii_case("content-encoding"))
}

pub(crate) const FORWARDED_HEADERS: &[&str] = &[
    "content-type",
    "content-encoding",
    "mcp-session-id",
    "x-request-id",
    "x-oai-request-id",
    "cf-ray",
    "x-openai-authorization-error",
    "x-error-json",
    "openai-organization",
    "openai-model",
    "openai-processing-ms",
    "openai-version",
    "x-models-etag",
    "x-reasoning-included",
    "anthropic-ratelimit-requests-limit",
    "anthropic-ratelimit-requests-remaining",
    "anthropic-ratelimit-tokens-limit",
    "anthropic-ratelimit-tokens-remaining",
    "retry-after",
    "x-ratelimit-limit-requests",
    "x-ratelimit-remaining-requests",
    "x-ratelimit-limit-tokens",
    "x-ratelimit-remaining-tokens",
    "cache-control",
];

pub(crate) fn is_forwarded_response_header(name: &str) -> bool {
    FORWARDED_HEADERS.contains(&name)
        || name.starts_with("x-codex-")
        || name.starts_with("x-ratelimit-")
}
