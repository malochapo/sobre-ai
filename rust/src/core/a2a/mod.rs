//! What is left of the agent-to-agent layer.
//!
//! The protocol itself — agent cards, the JSON-RPC compatibility shim, task and
//! transfer envelopes — is gone. These three have nothing to do with A2A and
//! only ever shared its folder:
//!
//! - [`cost_attribution`] — per-tool cost accounting, read by `core::gain`,
//!   `core::tool_health` and `server::post_dispatch`.
//! - [`rate_limiter`] — per-agent MCP request limiting, called by
//!   `server::dispatch`.
//! - [`message`] — the message shape `core::agents` stores.

pub mod cost_attribution;
pub mod message;
pub mod rate_limiter;
