//! Integration tests for the hardening sprint: the logging channel invariant.

// =============================================================================
// Contracts KV Block Integrity
// =============================================================================

mod contracts_integrity {
    /// Guards the load-bearing invariant: in MCP stdio mode `stdout`
    /// is the JSON-RPC transport, so every diagnostic MUST go to `stderr`. A stray
    /// switch to stdout would silently corrupt every MCP client's protocol stream.
    /// `tracing-subscriber` does not expose the runtime writer, so we assert on the
    /// logging single-source-of-truth instead.
    #[test]
    fn mcp_logging_targets_stderr_never_stdout() {
        let logging = include_str!("../../src/core/logging.rs");
        assert!(
            logging.contains("with_writer(std::io::stderr)"),
            "init_logging must pin the tracing writer to stderr"
        );
        assert!(
            !logging.contains("std::io::stdout"),
            "logging must never route tracing to stdout — it corrupts the MCP JSON-RPC channel"
        );
    }
}
