#![recursion_limit = "256"]
// Every `unsafe` block must carry a `// SAFETY:` comment justifying soundness.
// Enforced so the (mostly libc/Win32 syscall) unsafe surface stays documented.
#![warn(clippy::undocumented_unsafe_blocks)]
// `[dependencies]` is a hand-written table nothing linked to its users: nine
// crates stayed in it with no code using them. Flag the next one. Test builds
// are exempt, as dev-dependencies serve the integration tests, not the lib.
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[cfg(all(feature = "jemalloc", not(windows)))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "jemalloc", not(windows)))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "malloc_conf")]
pub static malloc_conf: &[u8] = b"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:1000\0";

// ---------------------------------------------------------------------------
// Pillar: Engine — context compression, MCP tools, agent hooks, local UX
// ---------------------------------------------------------------------------
pub mod compound_lexer;
pub mod core;
pub mod engine;
pub mod hook_handlers;
pub mod hooks;
pub mod instructions;
pub mod lsp;
pub mod marked_block;
pub mod mcp_stdio;
pub mod rewrite_registry;
pub mod server;
pub mod shell;
pub mod terminal_ui;
pub mod tool_defs;
pub mod tools;

// ---------------------------------------------------------------------------
// Pillar: Gateway — LLM API proxy, org-wide monitoring, budget enforcement
// ---------------------------------------------------------------------------
pub mod proxy;
pub mod proxy_autostart;
pub mod proxy_setup;

// ---------------------------------------------------------------------------
// Shared — CLI, IPC, config, diagnostics, setup
// ---------------------------------------------------------------------------
pub mod cli;
pub mod config_io;
pub mod doctor;
pub mod ipc;
pub mod setup;
pub mod status;
#[cfg(test)]
pub(crate) mod test_env;
pub mod uninstall;
