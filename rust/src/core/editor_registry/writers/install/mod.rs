// Auto-split from the former monolithic writers.rs. Grouped by operation
// (install) + shared helpers; behavior is unchanged.
//
// Split per-target to stay under the 1500-line LOC gate; every item is
// re-exported so `use install::*` in the parent resolves exactly as before.

mod claude;
mod codex;
mod copilot;
mod gemini;
mod opencode;
mod vscode;
mod zed;

// Glob re-exports: several writers/helpers are only exercised from the test
// module, and explicit `use` re-exports would trip `unused_imports` in
// non-test builds.
#[allow(clippy::wildcard_imports)]
pub(crate) use claude::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use codex::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use copilot::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use gemini::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use opencode::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use vscode::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use zed::*;
