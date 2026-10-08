//! Text extraction primitives: prose distillation, HTML to Markdown and PDF
//! text.
//!
//! - [`distill`] — prose compression, driven by the proxy (`proxy::compress`).
//! - [`html_to_text`] — HTML to Markdown, used by `core::extractors`.
//! - [`pdf`] — text extraction, used by `core::artifact_index`.

pub mod distill;
pub mod html_to_text;
pub mod pdf;
