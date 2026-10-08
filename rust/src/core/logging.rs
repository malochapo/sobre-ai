use tracing_subscriber::EnvFilter;

/// Initialize the tracing subscriber for CLI usage.
/// Respects `RUST_LOG` for filter control.
/// Defaults to `warn` level — keeps CLI output clean.
pub fn init_logging() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_string());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_ansi(crate::core::theme::stderr_color())
        .try_init();
}

/// Initialize logging for daemon/MCP mode (stderr, defaults to `info`).
/// Daemon logs go to a file, so verbosity is fine.
pub fn init_mcp_logging() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_ansi(crate::core::theme::stderr_color())
        .try_init();
}
