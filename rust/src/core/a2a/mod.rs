//! Per-tool cost accounting and per-agent request limiting.
//!
//! - [`cost_attribution`] — per-tool cost accounting, read by `core::gain`,
//!   `core::tool_health` and `server::post_dispatch`.
//! - [`rate_limiter`] — per-agent MCP request limiting, called by
//!   `server::dispatch`.

pub mod cost_attribution;
pub mod rate_limiter;
