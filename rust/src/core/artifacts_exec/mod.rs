//! Managed downloads of third-party binaries: the ONNX Runtime the `dense`
//! feature provisions on request.
//!
//! - [`binhash`] — SHA-256 of a file on disk.
//! - [`ort_provision`] — the ONNX Runtime pin, download and install location.

pub mod binhash;
pub mod ort_provision;
