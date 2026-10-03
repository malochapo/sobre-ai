//! Shell activation mode — controls when lean-ctx aliases auto-activate.

use serde::{Deserialize, Serialize};

use super::Config;

/// Controls when the shell hook auto-activates command aliases.
///
/// - `AgentsOnly`: (Default since #699) Aliases only activate when an AI agent
///   env var is detected (`LEAN_CTX_AGENT`, `CURSOR_AGENT`, `CLAUDECODE`,
///   `CODEBUDDY`, `CODEX_CLI_SESSION`, `GEMINI_SESSION`). lean-ctx exists to
///   save *agent* tokens — in a plain human terminal the aliases add overhead
///   and surface allowlist diagnostics with no benefit (GH #699).
/// - `Always`: Aliases are active in every interactive shell — the pre-#699
///   default, still available for `lean-ctx initped` fans who want their own
///   shell usage tracked.
/// - `Off`: Aliases never auto-activate. The user must call `lean-ctx-on` manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ShellActivation {
    Always,
    #[default]
    AgentsOnly,
    Off,
}

impl ShellActivation {
    pub fn from_env() -> Option<Self> {
        std::env::var("LEAN_CTX_SHELL_ACTIVATION")
            .ok()
            .and_then(|v| match v.trim().to_lowercase().as_str() {
                "always" => Some(Self::Always),
                "agents-only" | "agents_only" | "agentsonly" => Some(Self::AgentsOnly),
                "off" | "none" | "manual" => Some(Self::Off),
                _ => None,
            })
    }

    pub fn effective(config: &Config) -> Self {
        if let Some(env_val) = Self::from_env() {
            return env_val;
        }
        config.shell_activation.clone()
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
    fn serde_roundtrip() {
        let toml_str = r#"shell_activation = "agents-only""#;
        #[derive(Deserialize)]
        struct Wrapper {
            shell_activation: ShellActivation,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.shell_activation, ShellActivation::AgentsOnly);
    }
}
