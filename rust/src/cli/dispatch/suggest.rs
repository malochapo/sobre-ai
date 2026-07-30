//! "Did you mean?" suggestions for mistyped top-level CLI commands.
//!
//! Every name here must dispatch. A name that does not costs more than a
//! missing one: the user types something close to it, gets it suggested, runs
//! it, and lands on "unknown command". This list previously carried 38 such
//! names left behind by removed features.
//!
//! `completions::spec` holds the same names with their flags, and
//! `command_tree_covers_known_commands` enforces that this list is a subset of
//! that tree, so the two cannot drift apart in one direction.

/// Top-level command names that have a dispatch arm, plus their real aliases.
pub(crate) const KNOWN_COMMANDS: &[&str] = &[
    "gain",
    "learning",
    "health",
    "policy",
    "plugins",
    "addons",
    "rules",
    "eval",
    "benchmark",
    "verify-cache",
    "visualize",
    "audit",
    "index",
    "repomap",
    "cep",
    "dashboard",
    "provider",
    "watch",
    "proxy",
    "daemon",
    "init",
    "wrap",
    "unwrap",
    "status",
    "read",
    "call",
    "diff",
    "grep",
    "glob",
    "ls",
    "deps",
    "ghost",
    "filter",
    "graph",
    "smells",
    "session",
    "knowledge",
    "compact",
    "profile",
    "tools",
    "config",
    "allow",
    "untrust",
    "introspect",
    "cache",
    "theme",
    "tee",
    "terse",
    "restart",
    "stop",
    "dev-install",
    "codesign-setup",
    "doctor",
    "harden",
    "export-rules",
    "completions",
    "__complete",
    "gotchas",
    "learn",
    "help",
    "mcp",
];

/// Returns the closest known command to `input`, or `None` when nothing is
/// near enough to suggest with confidence. Distance + budget live in the
/// shared [`crate::core::levenshtein`] helper (#712), so CLI commands, config
/// keys and MCP tool names all suggest with identical semantics.
pub(super) fn closest_command(input: &str) -> Option<&'static str> {
    crate::core::levenshtein::closest(input, KNOWN_COMMANDS.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_typos() {
        assert_eq!(closest_command("doctr"), Some("doctor"));
        assert_eq!(closest_command("statuss"), Some("status"));
        assert_eq!(closest_command("upgrad"), Some("upgrade"));
    }

    #[test]
    fn exact_match_returns_itself() {
        assert_eq!(closest_command("read"), Some("read"));
        assert_eq!(closest_command("doctor"), Some("doctor"));
    }

    #[test]
    fn rejects_unrelated_input() {
        assert_eq!(closest_command("xyzzyplughfoo"), None);
        assert_eq!(closest_command(""), None);
        assert_eq!(closest_command("   "), None);
    }
}
