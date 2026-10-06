//! Text extraction primitives that outlived the web-research layer.
//!
//! This module used to turn a URL into compressed, citation-backed context via
//! `read_url` and the `ctx_url_read` tool. Both are gone; what stayed is used
//! by callers that never touch the network:
//!
//! - [`distill`] — prose compression, driven by the proxy (`proxy::compress`,
//!   `proxy::prose`, `proxy::prose_ranker`) and `core::extractive`.
//! - [`html_to_text`] — HTML to Markdown, used by `core::extractors`.
//! - [`pdf`] — text extraction, used by `core::artifact_index`.
//!
//! The folder name is now a misnomer; the pieces belong with their callers.

pub mod distill;
pub mod html_to_text;
pub mod pdf;
