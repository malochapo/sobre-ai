//! Pure tool-visibility policy for the MCP `tools/list` response.
//!
//! Extracted from the (async, server-bound) `list_tools` handler so the policy
//! is unit-testable in isolation. Nine axes used to narrow the surface here;
//! invariant 3 removed every one of them, so what is left is the rule that
//! internal/meta tools are never advertised.

/// The advertised surface, with live description compression. Offline
/// counterpart of the `tools/list` handler for `doctor overhead` and
/// `ContextOverhead::measure`, so measurement cannot drift from what the
/// server actually serves.
///
/// Invariant 3: this is the registry, unfiltered. Nine axes used to narrow it
/// here and in `list_tools`; keeping the two in step by hand is exactly how
/// the default surface silently fell to ten tools out of nineteen.
#[must_use]
pub fn advertised_tool_defs_default() -> Vec<rmcp::model::Tool> {
    let cfg = crate::core::config::Config::load();
    let level = crate::core::config::CompressionLevel::effective_configured(&cfg);
    let mode = crate::core::terse::mcp_compress::DescriptionMode::from_compression_level(&level);
    crate::server::registry::build_registry()
        .tool_defs()
        .into_iter()
        .map(|mut t| {
            if mode != crate::core::terse::mcp_compress::DescriptionMode::Full {
                let compressed = crate::core::terse::mcp_compress::compress_description(
                    t.name.as_ref(),
                    t.description.as_deref().unwrap_or(""),
                    mode,
                );
                t.description = Some(compressed.into());
            }
            t
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// Invariant 1: the cap is twenty, and nothing may raise it from outside.
    ///
    /// Previously unenforced anywhere in the tree — the number lived only in
    /// `INVARIANTS.md`.
    #[test]
    fn registry_stays_within_the_twenty_tool_cap() {
        let n = crate::server::registry::build_registry().tool_defs().len();
        assert!(
            n <= 20,
            "the MCP surface is capped at 20 tools; the registry now has {n}.              Adding one requires removing one (invariant 1)."
        );
    }

    /// Invariant 3: exactly one answer to `tools/list`, and it is the registry.
    ///
    /// This is the guard the project lacked. `CORE_TOOL_NAMES` narrowed the
    /// advertised set by hand and kept naming `shell` and `ctx_call` after both
    /// left the registry, so the default surface served ten tools out of
    /// nineteen with no error anywhere.
    #[test]
    fn advertised_surface_is_the_whole_registry() {
        let registered: std::collections::BTreeSet<String> =
            crate::server::registry::build_registry()
                .tool_defs()
                .into_iter()
                .map(|t| t.name.to_string())
                .collect();
        let advertised: std::collections::BTreeSet<String> = super::advertised_tool_defs_default()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(
            registered, advertised,
            "every registered tool must be advertised, and nothing else"
        );
    }
}
