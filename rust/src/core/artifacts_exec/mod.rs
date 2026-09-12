//! Running and verifying third-party artefacts on this machine.
//!
//! Everything sobre executes or loads that it did not build itself passes
//! through here: MCP servers started by the local gateway, tree-sitter grammar
//! dynamic libraries, and the ONNX Runtime the `dense` feature downloads.
//!
//! This used to live in `core/addons` alongside a package marketplace, which
//! made the directory name a lie: three subsystems that have nothing to do
//! with addons — `mcp_catalog`, `signatures_ts::grammar_loader` and
//! `ort_environment` — depended on it, and a reading of the folder name alone
//! nearly deleted grammar loading.
//!
//! The four concerns, deliberately kept together because they are one pipeline:
//!
//! 1. **Declare** what an artefact may do — [`capabilities`], [`policy`],
//!    [`manifest`].
//! 2. **Verify** it is what it claims — [`binhash`], [`integrity`],
//!    [`revocation`], [`signing`].
//! 3. **Provision** it — [`artifact_install`], [`grammar_install`],
//!    [`grammar_manifest`], [`grammar_registry`], [`ort_provision`],
//!    [`store`].
//! 4. **Contain** it while it runs — [`sandbox`], [`env_scrub`], [`runtime`],
//!    [`meter`], [`trusted_keys`].

pub mod artifact_install;
pub mod binhash;
pub mod capabilities;
pub mod env_scrub;
pub mod grammar_install;
pub mod grammar_manifest;
pub mod grammar_registry;
pub mod ort_provision;
pub mod policy;
pub mod revocation;
pub mod runtime;
pub mod sandbox;
pub mod signing;
pub mod trusted_keys;
