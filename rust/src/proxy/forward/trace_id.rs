use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;

const TRACE_ID_HEADER: &str = "x-trace-id";

pub(super) fn extract_or_generate_trace_id(headers: &HeaderMap) -> String {
    headers
        .get(TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map_or_else(generate_trace_id, str::to_owned)
}

/// A fresh `tr-<uuid v4>` trace id.
fn generate_trace_id() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("CSPRNG unavailable");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    );
    format!("tr-{uuid}")
}

pub(super) fn inject_trace_id(response: &mut Response, trace_id: &str) {
    response.headers_mut().insert(
        TRACE_ID_HEADER,
        HeaderValue::from_str(trace_id).expect("generated trace ID is a valid header"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provided_trace_id_is_preserved() {
        let mut headers = HeaderMap::new();
        headers.insert(TRACE_ID_HEADER, HeaderValue::from_static("tr-provided"));
        assert_eq!(extract_or_generate_trace_id(&headers), "tr-provided");
    }

    #[test]
    fn missing_trace_id_is_generated() {
        assert!(extract_or_generate_trace_id(&HeaderMap::new()).starts_with("tr-"));
    }

    #[test]
    fn trace_id_is_injected_into_response() {
        let mut response = Response::new(axum::body::Body::empty());
        inject_trace_id(&mut response, "tr-test");
        assert_eq!(response.headers()[TRACE_ID_HEADER], "tr-test");
    }
}
