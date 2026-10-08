//! "Did you mean?" suggestions for mistyped top-level CLI commands.
//!
//! Every name here must dispatch, and every named dispatch arm must be here:
//! `known_commands_match_dispatch_arms` holds the two together. A name that
//! does not dispatch costs more than a missing one: the user types something
//! close to it, gets it suggested, runs it, and lands on "unknown command".

/// Top-level command names that have a dispatch arm. Flag forms (`-c`,
/// `--version`, `--help`) are left out: they make poor suggestions.
pub(crate) const KNOWN_COMMANDS: &[&str] = &[
    "init",
    "uninstall",
    "doctor",
    "config",
    "gain",
    "index",
    "embeddings",
    "proxy",
    "call",
    "raw",
    "hook",
    "mcp",
    "help",
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
        assert_eq!(closest_command("indx"), Some("index"));
        assert_eq!(closest_command("doctro"), Some("doctor"));
        assert_eq!(closest_command("inti"), Some("init"));
    }

    #[test]
    fn exact_match_returns_itself() {
        assert_eq!(closest_command("gain"), Some("gain"));
        assert_eq!(closest_command("doctor"), Some("doctor"));
    }

    #[test]
    fn rejects_unrelated_input() {
        assert_eq!(closest_command("xyzzyplughfoo"), None);
        assert_eq!(closest_command(""), None);
        assert_eq!(closest_command("   "), None);
    }

    /// The named arms of the top-level `match` in `dispatch/mod.rs` and
    /// `KNOWN_COMMANDS` are the same set: no suggestion without an arm, no
    /// command the suggestions cannot reach.
    #[test]
    fn known_commands_match_dispatch_arms() {
        let src = include_str!("mod.rs");
        let start = src
            .find("match args[1].as_str() {")
            .expect("top-level command match");
        let end = start + src[start..].find("\n        }\n").expect("end of match");
        let mut arms: Vec<&str> = src[start..end]
            .lines()
            // Top-level arms sit at twelve spaces; nested matches go deeper.
            .filter(|l| l.starts_with("            \"") && l.contains("=>"))
            .flat_map(|l| l.split("=>").next().unwrap_or_default().split('|'))
            .map(|name| name.trim().trim_matches('"'))
            .filter(|name| !name.starts_with('-'))
            .collect();
        arms.sort_unstable();
        let mut known = KNOWN_COMMANDS.to_vec();
        known.sort_unstable();
        assert_eq!(arms, known);
    }
}
