mod provider;
pub(crate) use provider::*;
mod proxy;
pub(crate) use proxy::*;
#[cfg(test)]
mod tests;

/// True when the args ask for help anywhere (`--help`/`-h`/`help`).
/// Subcommand handlers must check this BEFORE executing: `lean-ctx daemon
/// enable --help` must print help, not install the service (GH #393).
pub(super) fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--help" || a == "-h" || a == "help")
}
