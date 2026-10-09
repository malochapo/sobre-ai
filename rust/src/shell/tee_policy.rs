//! Single source of truth for the shell "tee" decision — whether the full,
//! pre-compression output is saved so the agent can get it back with
//! `sobre_expand` instead of re-running the command.
//!
//! Both the CLI buffered path (`shell::pipeline`) and the MCP `sobre_shell`
//! handler call [`should_tee`]. The rule is simply "compression cut something":
//! whatever the agent did not see must stay retrievable.

/// Save the full output whenever the inline output does not contain all of it.
/// A blank output or a bare timeout notice has nothing to recover.
pub(crate) fn should_tee(
    full_output: &str,
    inline_output: &str,
    timeout_notice_only: bool,
) -> bool {
    !full_output.trim().is_empty()
        && !timeout_notice_only
        && output_was_elided(full_output, inline_output)
}

/// True only when the inline representation does not contain the complete
/// non-blank output. Wrappers such as `<error>…</error>` do not count as
/// elision.
fn output_was_elided(full_output: &str, inline_output: &str) -> bool {
    let full_output = full_output.trim_end_matches(['\r', '\n']);
    !full_output.is_empty() && !inline_output.contains(full_output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tees_exactly_when_content_was_cut() {
        assert!(should_tee(
            "first\nmissing\nlast\n",
            "first\n…\nlast",
            false
        ));
        assert!(!should_tee("hi\n", "<error>hi\n[exit:1]</error>", false));
        assert!(!should_tee("  \n", "", false));
        assert!(!should_tee("[timeout]", "", true));
    }
}
