//! Text extraction primitives: prose distillation and PDF text.
//!
//! - [`distill`] — prose compression, driven by the proxy (`proxy::compress`).
//! - [`pdf`] — PDF text, used by `core::extractors` for the search index.

pub mod distill;
pub mod pdf;
