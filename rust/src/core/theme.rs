//! Terminal text helpers: the three ANSI attributes the CLI uses, switched off
//! under `NO_COLOR` or when stdout is not a terminal.

use std::io::IsTerminal;

fn no_color() -> bool {
    std::env::var("NO_COLOR").is_ok() || !std::io::stdout().is_terminal()
}

pub fn rst() -> &'static str {
    if no_color() { "" } else { "\x1b[0m" }
}

pub fn bold() -> &'static str {
    if no_color() { "" } else { "\x1b[1m" }
}

pub fn dim() -> &'static str {
    if no_color() { "" } else { "\x1b[2m" }
}
