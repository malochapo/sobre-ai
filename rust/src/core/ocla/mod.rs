//! Open Context & Token Lifecycle Architecture (OCLA) public OSS contract.
//!
//! OCLA exposes local, provider-neutral control points. Implementations remain
//! in the engine or an OSS extension; commercial systems may consume this
//! versioned boundary but must never become a data-plane dependency.

pub mod budget;
pub mod budget_gate;
pub mod builtin;
pub mod capsule;
pub mod content_port;
pub mod health;
pub mod registry;
pub mod response_cache;
pub mod routing_quality;
pub mod tracing;
pub mod traits;
pub mod types;
pub mod unified_ledger;
pub mod wire;
pub mod wire_stream;

pub use registry::OclaRegistry;
pub use traits::*;
pub use types::*;
