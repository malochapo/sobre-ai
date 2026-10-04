//! Per-request markers the auth guard attaches for the forward path.
//!
//! A caller may tag a request with a project through the `x-leanctx-project`
//! header; the tag is booked on the usage record. The header is internal:
//! it is not on `ALLOWED_REQUEST_HEADERS`, so it never reaches the upstream.

/// Tags attached to an authenticated request. Inserted as a request extension
/// by the auth guard and stamped onto the usage record by the forward path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestTags {
    pub project: Option<String>,
}
