//! Shared interactive-confirmation helpers for consequential CLI writes.
//!
//! One governance — *show the consequence, then require explicit approval* —
//! guards every state-mutating write that can clobber existing state
//! (knowledge overwrites, behavior-changing `config set`).
//!
//! Invariant: with no TTY and no `--yes` we **refuse** rather than silently
//! apply. An automated/unattended run must never weaken state without an
//! explicit opt-in flag.

use std::io::{IsTerminal, Write};

const BOLD: &str = "\x1b[1m";
const YELLOW: &str = "\x1b[33m";
const RST: &str = "\x1b[0m";

/// Confirm a consequential change.
///
/// - `assume_yes` short-circuits to `true` (for `--yes` / scripted use).
/// - On a TTY we prompt `[y/N]` and accept only `y`/`yes`.
/// - With **no** TTY and **no** `--yes` we print a refusal hint and return
///   `false`, so the caller leaves state untouched.
pub(crate) fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        crate::errln!(
            "{YELLOW}Refusing to apply a consequential change non-interactively.{RST} Re-run with {BOLD}--yes{RST} to confirm."
        );
        return false;
    }
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_assume_yes_short_circuits() {
        assert!(confirm("anything", true));
    }
}
