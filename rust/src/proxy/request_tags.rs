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

/// Set only after the proxy auth guard accepts the proxy's own token (or in
/// loopback-open mode). The provider API-key fallback must not be able to
/// manufacture managed lineage headers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AuthenticatedRequest;
