//! Shared tool lifecycle — records the `sobre -c` shell wrapper's compressions
//! in the usage ledger, as the MCP server records its own.

use crate::core::usage::{self, Surface};

/// Record a shell command's compression in the usage ledger, filed under the
/// command's program name (`cargo`, `git`).
pub fn record_shell_command(command: &str, original_tokens: usize, output_tokens: usize) {
    usage::record_compression(
        Surface::Shell,
        program_name(command),
        original_tokens,
        output_tokens,
    );
}

/// First word of a command line, without its directory: `/usr/bin/git log`
/// gives `git`.
pub(crate) fn program_name(command: &str) -> &str {
    let first = command.split_whitespace().next().unwrap_or("");
    first.rsplit(['/', '\\']).next().unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The record_* paths read the data-dir env, so every test here takes the
    // shared isolation lock and keeps its disk writes inside a throwaway dir.

    // An unreadable path falls back to the caller's count, so the ratio is
    // exactly delivered/original.

    #[test]
    fn record_shell_does_not_panic_without_session() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        record_shell_command("cargo test", 500, 200);
    }

    #[test]
    fn program_name_is_the_first_word_without_directory() {
        assert_eq!(program_name("/usr/bin/git log -5"), "git");
        assert_eq!(program_name("cargo test"), "cargo");
        assert_eq!(program_name(""), "");
    }
}
