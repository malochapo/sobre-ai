//! Request-body preparation: decode, compress, re-encode.

use axum::http::{StatusCode, request::Parts};
use std::borrow::Cow;

use crate::proxy::codec::{
    RequestBodyEncoding, decode_gzip_bounded, decode_zstd_bounded, encode_gzip, encode_zstd,
    request_body_encoding,
};

use super::max_body_bytes;

/// Records the proxy's compression of one request in the usage ledger.
///
/// Both bodies are tokenized whole, which takes too long to do before
/// forwarding, so it runs on its own thread and the request does not wait.
fn record_compression_off_path(before: Vec<u8>, after: Vec<u8>) {
    std::thread::spawn(move || {
        let count = |b: &[u8]| crate::core::tokens::count_tokens(&String::from_utf8_lossy(b));
        crate::core::usage::record_compression(
            crate::core::usage::Surface::Proxy,
            "request",
            count(&before),
            count(&after),
        );
    });
}

pub(crate) struct PreparedRequestBody {
    pub(crate) body: Vec<u8>,
    pub(crate) preserve_content_encoding: bool,
    /// The `model` the client asked for, when the body is JSON and names one.
    pub(crate) request_model: Option<String>,
}

pub(crate) fn prepare_request_body(
    parts: &Parts,
    body_bytes: &[u8],
    compress_body: impl FnOnce(serde_json::Value, usize) -> (Vec<u8>, usize, usize),
) -> Result<PreparedRequestBody, StatusCode> {
    let encoding = request_body_encoding(parts);
    let decoded = match encoding {
        RequestBodyEncoding::Identity => Cow::Borrowed(body_bytes),
        RequestBodyEncoding::Gzip => Cow::Owned(decode_gzip_bounded(body_bytes, max_body_bytes())?),
        RequestBodyEncoding::Zstd => Cow::Owned(decode_zstd_bounded(body_bytes, max_body_bytes())?),
        RequestBodyEncoding::Passthrough => {
            return Ok(PreparedRequestBody {
                body: body_bytes.to_vec(),
                preserve_content_encoding: true,
                request_model: None,
            });
        }
    };

    let Some(parsed) = serde_json::from_slice::<serde_json::Value>(&decoded).ok() else {
        return Ok(PreparedRequestBody {
            body: body_bytes.to_vec(),
            preserve_content_encoding: encoding != RequestBodyEncoding::Identity,
            request_model: None,
        });
    };

    let request_model = parsed
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let original_size = decoded.len();
    let (logical_body, _, compressed_size) = compress_body(parsed, original_size);
    if compressed_size < original_size {
        record_compression_off_path(decoded.to_vec(), logical_body.clone());
    }
    let body = match encoding {
        RequestBodyEncoding::Identity => logical_body,
        RequestBodyEncoding::Gzip => encode_gzip(&logical_body)?,
        RequestBodyEncoding::Zstd => encode_zstd(&logical_body)?,
        RequestBodyEncoding::Passthrough => unreachable!("passthrough returned above"),
    };

    Ok(PreparedRequestBody {
        body,
        preserve_content_encoding: encoding != RequestBodyEncoding::Identity,
        request_model,
    })
}
