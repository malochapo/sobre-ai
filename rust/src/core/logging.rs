use tracing_subscriber::EnvFilter;

/// Initialize the tracing subscriber on stderr. `RUST_LOG` wins; otherwise
/// `default_level` (`warn` keeps CLI output clean, the MCP server logs `info`).
pub fn init_logging(default_level: &str) {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_level.to_string());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_ansi(crate::core::theme::stderr_color())
        .try_init();
}
