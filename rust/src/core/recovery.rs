//! Recovery-hint policy + grammar.
//!
//! sobre compression is fully reversible, but agents only act on that if we
//! *show* the escape hatch — otherwise they re-read compressed output
//! line-by-line (the "too compressed" complaint). The `RECOVER` line of the MCP
//! instructions ([`crate::instructions::GUIDE`]) teaches the vocabulary once;
//! this module is the reactive half: it owns the canonical, **non-MCP-first**
//! phrasing so every compressed `sobre_read` view renders identical, byte-stable
//! wording.
//!
//! Determinism (#498): the footer is a pure function of its input path, never
//! of per-call session state. It is therefore "deduped by
//! mode" — only lossy/compressed views carry it; escalating to `full`/`raw` (the
//! recovery action itself) drops it — rather than by fragile session state that
//! would break the byte-stability guards.

/// The compressed-view header published verbatim in `tdd_schema`.
pub const COMPACT_VIEW_HEADER: &str =
    "[sobre: compact view — nothing lost, full source on request]";

/// Footer appended to a compressed `sobre_read` view: one line that leads with the
/// MCP-free path ("read the file directly") so orgs that forbid MCP still have a
/// route, then the `sobre_read` shortcut.
#[must_use]
pub fn read_footer(file_path: &str) -> String {
    format!(
        "[sobre: full source in \"{file_path}\" (read it directly), or sobre_read(\"{file_path}\", mode=\"full\")]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_footer_leads_with_native_path() {
        let minimal = read_footer("src/x.rs");
        assert!(minimal.lines().count() == 1, "minimal is a single line");
        // Non-MCP path must come before the MCP shortcut.
        let native = minimal.find("full source in \"src/x.rs\"").unwrap();
        let mcp = minimal.find("sobre_read(").unwrap();
        assert!(native < mcp, "native path must precede the MCP route");
        assert!(
            crate::server::registry::unregistered_tool_mentions(&minimal).is_empty(),
            "the minimal footer names an unregistered tool: {minimal}"
        );
    }

    /// Determinism (#498): the footer/clause are pure functions of their inputs,
    /// so repeated calls are byte-identical (provider prompt caching depends on it).
    #[test]
    fn footer_is_byte_stable() {
        assert_eq!(read_footer("src/a.rs"), read_footer("src/a.rs"));
    }
}
