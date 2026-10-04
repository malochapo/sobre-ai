//! Recovery-hint policy + grammar.
//!
//! lean-ctx compression is fully reversible, but agents only act on that if we
//! *show* the escape hatch — otherwise they re-read compressed output
//! line-by-line (the "too compressed" complaint). The proactive `RECOVER` rule
//! ([`crate::core::rules_canonical::RECOVER`]) teaches the vocabulary once in the
//! system prompt; this module is the reactive half: it resolves the effective
//! [`RecoveryHints`] tier and owns the canonical, **non-MCP-first** phrasings so
//! every compressed-output site (`ctx_read`, `ctx_shell` tee, archive / firewall
//! / spill handles) renders identical, byte-stable wording.
//!
//! Determinism (#498): the tier is a pure function of profile + config + env,
//! never of per-call session state, and every phrasing is a pure function of its
//! inputs (paths/ids are content-addressed). The footer is therefore "deduped by
//! mode" — only lossy/compressed views carry it; escalating to `full`/`raw` (the
//! recovery action itself) drops it — rather than by fragile session state that
//! would break the byte-stability guards.

use crate::core::config::{Config, RecoveryHints};

/// Header line of the `Full`-tier compressed-view footer. Single source of truth:
/// also surfaced verbatim in `tdd_schema` so the published schema and the runtime
/// affordance can never drift.
pub const COMPACT_VIEW_HEADER: &str =
    "[lean-ctx: compact view — nothing lost, full source on request]";

/// Resolve the effective recovery tier for the current call.
/// `config.recovery_hints` (default [`RecoveryHints::Minimal`]).
#[must_use]
pub fn tier() -> RecoveryHints {
    Config::load().recovery_hints
}

/// Footer appended to a compressed `ctx_read` view. Leads with the MCP-free path
/// ("read the file directly") so orgs that forbid MCP still have a route, then
/// the `ctx_*` shortcuts. `None` when the tier is `Off`.
#[must_use]
pub fn read_footer(file_path: &str) -> Option<String> {
    match tier() {
        RecoveryHints::Off => None,
        RecoveryHints::Minimal => Some(format!(
            "[lean-ctx] full source: read \"{file_path}\" directly (no MCP)  ·  or ctx_read(\"{file_path}\", mode=\"full\")"
        )),
        // The header line is the SSOT [`COMPACT_VIEW_HEADER`] (also in `tdd_schema`);
        // the ladder now leads with the native path before the MCP shortcuts.
        RecoveryHints::Full => Some(format!(
            "{COMPACT_VIEW_HEADER}\n  full: read \"{file_path}\" directly (no MCP)  ·  ctx_read(\"{file_path}\", mode=\"full\")  ·  exact bytes: ctx_read(\"{file_path}\", raw=true)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `recovery_hints` in config.toml pins the tier.
    #[test]
    fn config_pins_tier() {
        let iso = crate::core::data_dir::isolated_data_dir();
        std::fs::write(iso.path().join("config.toml"), "recovery_hints = \"off\"\n").unwrap();
        assert_eq!(tier(), RecoveryHints::Off);
        std::fs::write(
            iso.path().join("config.toml"),
            "recovery_hints = \"full\"\n",
        )
        .unwrap();
        assert_eq!(tier(), RecoveryHints::Full);
    }

    #[test]
    fn read_footer_leads_with_native_path_and_respects_off() {
        let iso = crate::core::data_dir::isolated_data_dir();
        std::fs::write(iso.path().join("config.toml"), "recovery_hints = \"off\"\n").unwrap();
        assert!(
            read_footer("src/x.rs").is_none(),
            "off suppresses the footer"
        );

        std::fs::write(
            iso.path().join("config.toml"),
            "recovery_hints = \"minimal\"\n",
        )
        .unwrap();
        let minimal = read_footer("src/x.rs").expect("minimal emits a footer");
        assert!(minimal.lines().count() == 1, "minimal is a single line");
        // Non-MCP path must come before the MCP shortcut.
        let native = minimal.find("read \"src/x.rs\" directly").unwrap();
        let mcp = minimal.find("ctx_read(").unwrap();
        assert!(native < mcp, "native path must precede the MCP route");

        std::fs::write(
            iso.path().join("config.toml"),
            "recovery_hints = \"full\"\n",
        )
        .unwrap();
        let full = read_footer("src/x.rs").expect("full emits a footer");
        assert!(full.contains("raw=true"));
        assert!(
            crate::server::registry::unregistered_tool_mentions(&full).is_empty(),
            "the full footer names an unregistered tool: {full}"
        );
        assert!(
            crate::server::registry::unregistered_tool_mentions(&minimal).is_empty(),
            "the minimal footer names an unregistered tool: {minimal}"
        );
        assert!(
            full.find("read \"src/x.rs\" directly").unwrap() < full.find("raw=true").unwrap(),
            "full ladder still leads with the native path"
        );
    }

    /// Determinism (#498): the footer/clause are pure functions of their inputs,
    /// so repeated calls are byte-identical (provider prompt caching depends on it).
    #[test]
    fn footer_is_byte_stable() {
        let iso = crate::core::data_dir::isolated_data_dir();
        std::fs::write(
            iso.path().join("config.toml"),
            "recovery_hints = \"minimal\"\n",
        )
        .unwrap();
        assert_eq!(read_footer("src/a.rs"), read_footer("src/a.rs"));
    }
}
