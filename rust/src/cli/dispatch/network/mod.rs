use crate::tui;

mod provider;
pub(crate) use provider::*;
mod proxy;
pub(crate) use proxy::*;
#[cfg(test)]
mod tests;

pub(super) fn cmd_watch(rest: &[String]) {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: lean-ctx watch");
        println!("  Live TUI dashboard (real-time event stream).");
        return;
    }
    if let Err(e) = tui::run() {
        tracing::error!("TUI error: {e}");
        std::process::exit(1);
    }
}

/// True when the args ask for help anywhere (`--help`/`-h`/`help`).
/// Subcommand handlers must check this BEFORE executing: `lean-ctx daemon
/// enable --help` must print help, not install the service (GH #393).
pub(super) fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--help" || a == "-h" || a == "help")
}
