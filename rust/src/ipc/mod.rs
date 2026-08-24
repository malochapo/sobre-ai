//! Process inspection helpers.
//!
//! This module also carried the daemon's IPC endpoint — a `DaemonAddr` over a
//! Unix socket or a Windows named pipe, bound by `http_server::serve_ipc`. The
//! daemon is gone (sobre is a CLI, and every command that delegated to it
//! already had a local fallback), so only the process helpers remain. The name
//! is now wider than the contents; folding `process` up a level is future work.

pub mod process;
