mod proxy;
pub(crate) use proxy::*;
#[cfg(test)]
mod tests;

/// True when the args ask for help anywhere (`--help`/`-h`/`help`).
/// Subcommand handlers must check this BEFORE executing: `sobre proxy
/// enable --help` must print help, not install the service.
pub(super) fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--help" || a == "-h" || a == "help")
}
