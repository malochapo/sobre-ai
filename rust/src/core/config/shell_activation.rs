//! Shell activation mode — controls when lean-ctx aliases auto-activate.
//!
//! Chosen at `init --shell-activation <mode>` and baked into the generated
//! shell hook, which is where it is read back: no config key.

use std::sync::OnceLock;

/// Controls when the shell hook auto-activates command aliases.
///
/// - `AgentsOnly`: (Default since #699) Aliases only activate when an AI agent
///   env var is detected (`LEAN_CTX_AGENT`, `CURSOR_AGENT`, `CLAUDECODE`,
///   `CODEX_CLI_SESSION`, `GEMINI_SESSION`). lean-ctx exists to
///   save *agent* tokens — in a plain human terminal the aliases add overhead
///   and surface allowlist diagnostics with no benefit (GH #699).
/// - `Always`: Aliases are active in every interactive shell.
/// - `Off`: Aliases never auto-activate. The user must call `lean-ctx-on` manually.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShellActivation {
    Always,
    #[default]
    AgentsOnly,
    Off,
}

/// Comment line every generated shell hook carries, followed by the mode.
pub const HOOK_MARKER: &str = "# lean-ctx shell-activation: ";

/// The hook files `init` writes, in the config dir.
const HOOK_FILES: &[&str] = &[
    "shell-hook.zsh",
    "shell-hook.bash",
    "shell-hook.fish",
    "shell-hook.ps1",
];

static CLI_OVERRIDE: OnceLock<ShellActivation> = OnceLock::new();

impl ShellActivation {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "always" => Some(Self::Always),
            "agents-only" => Some(Self::AgentsOnly),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::AgentsOnly => "agents-only",
            Self::Off => "off",
        }
    }

    /// Record the mode passed to `init --shell-activation` for this process.
    pub fn set_cli_override(mode: Self) {
        let _ = CLI_OVERRIDE.set(mode);
    }

    /// The mode to bake into a hook: the `init` flag, else the mode the
    /// installed hook already carries, else the default.
    #[must_use]
    pub fn resolve() -> Self {
        CLI_OVERRIDE
            .get()
            .copied()
            .or_else(Self::installed)
            .unwrap_or_default()
    }

    /// The mode baked into the installed shell hook, if any.
    #[must_use]
    pub fn installed() -> Option<Self> {
        let dir = crate::core::paths::config_dir().ok()?;
        HOOK_FILES.iter().find_map(|name| {
            let content = std::fs::read_to_string(dir.join(name)).ok()?;
            Self::from_hook(&content)
        })
    }

    /// The mode a generated hook carries in its [`HOOK_MARKER`] line.
    #[must_use]
    pub fn from_hook(content: &str) -> Option<Self> {
        content
            .lines()
            .find_map(|line| line.strip_prefix(HOOK_MARKER))
            .and_then(Self::parse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GH #699: lean-ctx must be transparent in a plain human terminal —
    /// aliases (and their allowlist diagnostics) only belong in agent
    /// sessions unless the user explicitly opts into `always`.
    #[test]
    fn default_is_agents_only() {
        assert_eq!(ShellActivation::default(), ShellActivation::AgentsOnly);
    }

    #[test]
    fn as_str_roundtrips_through_parse() {
        for mode in [
            ShellActivation::Always,
            ShellActivation::AgentsOnly,
            ShellActivation::Off,
        ] {
            assert_eq!(ShellActivation::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(ShellActivation::parse("sometimes"), None);
    }

    #[test]
    fn from_hook_reads_the_marker_line() {
        let hook = format!("# lean-ctx shell hook\n{HOOK_MARKER}always\n_lc() {{ :; }}\n");
        assert_eq!(
            ShellActivation::from_hook(&hook),
            Some(ShellActivation::Always)
        );
        assert_eq!(ShellActivation::from_hook("no marker here"), None);
    }
}
