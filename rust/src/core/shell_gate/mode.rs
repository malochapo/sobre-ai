//! Shell-security mode — the master switch for ctx_shell / `lean-ctx -c` command
//! gating (GL #788).
//!
//! Three levels, applied at the single chokepoint
//! [`check_shell_command`](super::check_shell_command) so MCP and CLI behave
//! identically:
//!
//! - `enforce` (**default**, secure-by-default): the unconditional blocks and the
//!   unconditional/dangerous-pattern blocks are enforced — today's behaviour.
//! - `warn`: the same checks run but a violation is only logged (tracing),
//!   never blocked.
//! - `off`: command gating is skipped entirely (unconditional blocks, dangerous patterns,
//!   `eval`/`exec`/interpreter `-c`). A deliberate, documented opt-out for power
//!   users who accept the risk — **compression stays fully active**.
//! Resolution: `shell_security` in `config.toml`, else
//! [`ShellSecurity::Enforce`].
//! The default is `enforce` on purpose: lean-ctx mediates the agent's shell, so
//! defaulting to anything weaker would silently downgrade security for every
//! existing install on upgrade.

/// Active shell-security posture. Order is least → most permissive only for
/// readability; do not rely on ordinal values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellSecurity {
    /// Block disallowed commands (today's behaviour). Secure-by-default.
    #[default]
    Enforce,
    /// Run every check but only log violations — never block.
    Warn,
    /// Skip command gating entirely. Compression is unaffected.
    Off,
}

impl ShellSecurity {
    /// Parse a config/env value leniently. Returns `None` for unknown text so
    /// the caller can fall back to the secure default instead of failing.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "enforce" | "block" | "strict" | "on" => Some(Self::Enforce),
            "warn" | "warn-only" | "warn_only" => Some(Self::Warn),
            "off" | "disabled" | "none" | "yolo" => Some(Self::Off),
            _ => None,
        }
    }

    /// Canonical lower-case name (matches the config value).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Warn => "warn",
            Self::Off => "off",
        }
    }

    /// Resolve the active mode: config → secure default.
    pub fn resolve() -> Self {
        crate::core::config::Config::load()
            .shell_security
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or_default()
    }
}

impl std::fmt::Display for ShellSecurity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_canonical_and_aliases() {
        assert_eq!(
            ShellSecurity::parse("enforce"),
            Some(ShellSecurity::Enforce)
        );
        assert_eq!(ShellSecurity::parse("ON"), Some(ShellSecurity::Enforce));
        assert_eq!(ShellSecurity::parse(" Warn "), Some(ShellSecurity::Warn));
        assert_eq!(ShellSecurity::parse("off"), Some(ShellSecurity::Off));
        assert_eq!(ShellSecurity::parse("yolo"), Some(ShellSecurity::Off));
    }

    #[test]
    fn parse_rejects_unknown_so_caller_can_default() {
        assert_eq!(ShellSecurity::parse("loose"), None);
        assert_eq!(ShellSecurity::parse(""), None);
    }

    #[test]
    fn default_is_enforce() {
        assert_eq!(ShellSecurity::default(), ShellSecurity::Enforce);
    }

    #[test]
    fn as_str_roundtrips_through_parse() {
        for mode in [
            ShellSecurity::Enforce,
            ShellSecurity::Warn,
            ShellSecurity::Off,
        ] {
            assert_eq!(ShellSecurity::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn resolve_reads_config_and_tolerates_garbage() {
        let iso = crate::core::data_dir::isolated_data_dir();
        let cfg = iso.path().join("config.toml");
        std::fs::write(&cfg, "shell_security = \"off\"\n").unwrap();
        assert_eq!(ShellSecurity::resolve(), ShellSecurity::Off);
        std::fs::write(&cfg, "shell_security = \"garbage\"\n").unwrap();
        // Unknown config value → the default, never panics.
        assert_eq!(ShellSecurity::resolve(), ShellSecurity::default());
    }
}
