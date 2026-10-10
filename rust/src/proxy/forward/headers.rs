//! Which headers the proxy passes on: all of them, both ways, but those that
//! describe one connection rather than the message, and the few it must
//! set itself.

/// Hop-by-hop headers (RFC 9110 §7.6.1): they describe one connection, never
/// the message, so no proxy forwards them. Any `proxy-*` header too.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// True for a hop-by-hop header, or one the message's `Connection` header
/// names (`connection_names`, lower-case).
fn is_hop_by_hop(name: &str, connection_names: &[String]) -> bool {
    HOP_BY_HOP.contains(&name)
        || name.starts_with("proxy-")
        || connection_names.iter().any(|n| n == name)
}

/// The header names a `Connection` header lists, lower-case: they are
/// hop-by-hop for that message.
pub(crate) fn connection_names(headers: &axum::http::HeaderMap) -> Vec<String> {
    headers
        .get_all(axum::http::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Whether a request header (lower-case `name`) goes upstream. Everything
/// does but hop-by-hop headers; `host` and `content-length`, which the
/// client sets for the upstream and the body it sends; `accept-encoding`, so
/// the answer comes uncompressed and the proxy can read the usage it
/// reports; and `content-encoding` unless the body goes as it came
/// (`preserve_content_encoding`).
pub(crate) fn should_forward_request_header(
    name: &str,
    preserve_content_encoding: bool,
    connection_names: &[String],
) -> bool {
    match name {
        "host" | "content-length" | "accept-encoding" => false,
        "content-encoding" => preserve_content_encoding,
        _ => !is_hop_by_hop(name, connection_names),
    }
}

/// Whether a response header (lower-case `name`) goes back to the client:
/// all but hop-by-hop headers and `content-length`, which the proxy sets for
/// the body it sends.
pub(crate) fn is_forwarded_response_header(name: &str, connection_names: &[String]) -> bool {
    name != "content-length" && !is_hop_by_hop(name, connection_names)
}
