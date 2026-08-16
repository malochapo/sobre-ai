use std::fmt;

/// Controls which MCP tools are exposed to agents.
///
/// Three built-in tiers reduce tool-list overwhelm for new users
/// while letting power users keep everything.
///
/// When NO profile is pinned (no config key, no env var), the server
/// advertises only the lazy core set (`CORE_TOOL_NAMES`) and the
/// effective profile falls back to `Power` — which acts as a pure call-gate
/// ("everything reachable via ctx_call"), not as an advertisement list.
/// Pinning a profile makes the advertised set explicit and authoritative
/// (#358), which costs schema tokens: `standard` advertises its 16 full
/// schemas, `power` the whole registry (#575).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolProfile {
    Minimal,
    Standard,
    Power,
    Custom(Vec<String>),
}

impl ToolProfile {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "minimal" | "min" => Some(Self::Minimal),
            "standard" | "std" | "default" => Some(Self::Standard),
            "power" | "full" | "all" => Some(Self::Power),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Minimal => "minimal",
            Self::Standard => "standard",
            Self::Power => "power",
            Self::Custom(_) => "custom",
        }
    }

    pub fn description(&self) -> &str {
        match self {
            Self::Minimal => "5 surgical tools — each irreplaceable (recommended)",
            Self::Standard => "16 balanced tools (adds compose, explore, callgraph, patch, more)",
            Self::Power => "All tools exposed",
            Self::Custom(v) => {
                if v.is_empty() {
                    "Custom tool list (empty)"
                } else {
                    "Custom tool list"
                }
            }
        }
    }


    /// Whether this tool may be named to the agent.
    ///
    /// Invariant 3 fixed the surface: `tools/list` is the whole registry, for
    /// every client, always. So the profile selects nothing and the only
    /// question left is whether the tool exists — which also stops the
    /// injected rules from teaching a name that was removed.
    #[must_use]
    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        crate::server::registry::build_registry().contains(tool_name)
    }

    /// Size of the advertised surface — the registry, whatever the variant.
    #[must_use]
    pub fn tool_count(&self) -> usize {
        crate::server::registry::build_registry().tool_defs().len()
    }

    /// Names of the advertised tools — the registry, whatever the variant.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        crate::server::registry::build_registry()
            .tool_defs()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    /// The one profile.
    ///
    /// Resolution used to run `LEAN_CTX_TOOL_PROFILE` > config `tool_profile`
    /// > `tools.enabled` > default. That cascade is what invariant 2 forbids,
    /// and it reached the tool surface, which invariant 3 forbids too.
    /// `Power` was the default and means "everything", so fixing it here
    /// changes nothing for anyone who had not pinned another.
    #[must_use]
    pub fn from_config(_cfg: &super::config::Config) -> Self {
        Self::Power
    }
}

impl fmt::Display for ToolProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}



/// Writes the `tool_profile` setting to config.toml, preserving all comments,
/// formatting, and unrelated keys (robust against substring/comment matches).
pub fn set_profile_in_config(profile_name: &str) -> Result<(), String> {
    // Canonical config location (RO-safe config category, GH #408). Writing it
    // anywhere else than `Config::load` reads would split-brain once the data
    // default flips to `$XDG_DATA_HOME`.
    let config_path = crate::core::config::Config::path()
        .ok_or_else(|| "Cannot determine config dir".to_string())?;

    let mut doc = crate::config_io::load_toml_document(&config_path);
    doc["tool_profile"] = toml_edit::value(profile_name);
    crate::config_io::write_toml_document(&config_path, &doc)?;
    Ok(())
}

/// Removes the `tool_profile` key from config.toml, restoring the lean
/// default: only the lazy core set is advertised in `tools/list`,
/// while every registered tool stays reachable through `ctx_call`. This is
/// the recommended low-overhead mode (#575).
pub fn clear_profile_in_config() -> Result<(), String> {
    let config_path = crate::core::config::Config::path()
        .ok_or_else(|| "Cannot determine config dir".to_string())?;
    if !config_path.exists() {
        return Ok(());
    }

    let mut doc = crate::config_io::load_toml_document(&config_path);
    if doc.remove("tool_profile").is_none() {
        return Ok(());
    }
    crate::config_io::write_toml_document(&config_path, &doc)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_profiles() {
        assert_eq!(ToolProfile::parse("minimal"), Some(ToolProfile::Minimal));
        assert_eq!(ToolProfile::parse("min"), Some(ToolProfile::Minimal));
        assert_eq!(ToolProfile::parse("standard"), Some(ToolProfile::Standard));
        assert_eq!(ToolProfile::parse("std"), Some(ToolProfile::Standard));
        assert_eq!(ToolProfile::parse("default"), Some(ToolProfile::Standard));
        assert_eq!(ToolProfile::parse("power"), Some(ToolProfile::Power));
        assert_eq!(ToolProfile::parse("full"), Some(ToolProfile::Power));
        assert_eq!(ToolProfile::parse("all"), Some(ToolProfile::Power));
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(ToolProfile::parse("MINIMAL"), Some(ToolProfile::Minimal));
        assert_eq!(ToolProfile::parse("Standard"), Some(ToolProfile::Standard));
        assert_eq!(ToolProfile::parse("POWER"), Some(ToolProfile::Power));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(ToolProfile::parse("unknown"), None);
        assert_eq!(ToolProfile::parse(""), None);
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(format!("{}", ToolProfile::Minimal), "minimal");
        assert_eq!(format!("{}", ToolProfile::Standard), "standard");
        assert_eq!(format!("{}", ToolProfile::Power), "power");
        assert_eq!(
            format!("{}", ToolProfile::Custom(vec!["ctx_read".into()])),
            "custom"
        );
    }

    /// Invariant 3. The tiers used to select different tool sets; they no
    /// longer select anything, so every variant must answer identically.
    #[test]
    fn every_variant_answers_the_same_surface() {
        let variants = [
            ToolProfile::Minimal,
            ToolProfile::Standard,
            ToolProfile::Power,
            ToolProfile::Custom(vec![]),
            ToolProfile::Custom(vec!["ctx_read".to_string()]),
        ];
        let names = variants[0].tool_names();
        assert!(!names.is_empty(), "the registry must not be empty");
        for v in &variants {
            assert_eq!(v.tool_names(), names, "{v} advertises a different surface");
            assert_eq!(v.tool_count(), names.len(), "{v} counts differently");
        }
    }

    /// `is_tool_enabled` is registry membership, not a profile filter — so a
    /// name that left the registry stops being taught by the injected rules.
    #[test]
    fn enabled_means_registered() {
        let p = ToolProfile::Power;
        assert!(p.is_tool_enabled("ctx_read"));
        assert!(!p.is_tool_enabled("nonexistent_tool"));
    }

    /// Invariant 2: no cascade. Neither the config key nor the env var that
    /// used to pin a tier may move the surface.
    #[test]
    fn from_config_ignores_config_and_env() {
        let _guard = crate::core::data_dir::test_env_lock();
        crate::test_env::set_var("LEAN_CTX_TOOL_PROFILE", "minimal");
        let cfg = crate::core::config::Config {
            tool_profile: Some("minimal".to_string()),
            tools_enabled: vec!["ctx_read".to_string()],
            ..Default::default()
        };
        let resolved = ToolProfile::from_config(&cfg);
        crate::test_env::remove_var("LEAN_CTX_TOOL_PROFILE");
        assert_eq!(resolved, ToolProfile::Power);
    }

    #[test]
    fn clear_profile_removes_key_and_is_idempotent() {
        let iso = crate::core::data_dir::isolated_data_dir();
        set_profile_in_config("power").unwrap();
        let config_path = iso.path().join("config.toml");
        assert!(
            std::fs::read_to_string(&config_path)
                .unwrap()
                .contains("tool_profile"),
            "set_profile_in_config must write the key"
        );

        clear_profile_in_config().unwrap();
        assert!(
            !std::fs::read_to_string(&config_path)
                .unwrap()
                .contains("tool_profile"),
            "clear_profile_in_config must remove the key"
        );

        // Idempotent: clearing again (and on a missing file) must not fail.
        clear_profile_in_config().unwrap();
    }

    #[test]
    fn clear_profile_on_missing_config_is_ok() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        clear_profile_in_config().unwrap();
    }
}
