//! What is left of the OCLA layer.
//!
//! The trait/registry/adapter indirection is gone: each of the fourteen
//! capabilities either wrapped a module the caller can reach directly, or kept
//! a ring buffer whose only readers were the dashboard's SSE routes.
//!
//! What remains here is not an architecture, it is four modules that happen to
//! share a folder — `response_cache` and `routing_quality` serve the proxy,
//! `unified_ledger` serves the savings ledger, and `types` carries the request
//! context they all pass around. The folder name no longer means anything;
//! these four belong with their callers.

pub mod response_cache;
pub mod routing_quality;
pub mod types;
pub mod unified_ledger;

pub use types::*;
