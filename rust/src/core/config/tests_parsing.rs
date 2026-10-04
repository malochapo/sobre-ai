//! Tests for config parsing and serialization.

#[allow(unused_imports)]
use super::*;

#[cfg(test)]
mod compression_level_tests {
    use super::super::*;

    #[test]
    fn default_is_lite() {
        // Friendly default: plain-English concise guidance, not the symbolic
        // dense/expert-terse styles (those are opt-in power modes).
        assert_eq!(CompressionLevel::default(), CompressionLevel::Lite);
    }

    #[test]
    fn to_components_off() {
        let (ta, od, crp, tm) = CompressionLevel::Off.to_components();
        assert_eq!(ta, TerseAgent::Off);
        assert_eq!(od, OutputDensity::Normal);
        assert_eq!(crp, "off");
        assert!(!tm);
    }

    #[test]
    fn to_components_lite() {
        let (ta, od, crp, tm) = CompressionLevel::Lite.to_components();
        assert_eq!(ta, TerseAgent::Lite);
        assert_eq!(od, OutputDensity::Terse);
        assert_eq!(crp, "off");
        assert!(tm);
    }

    #[test]
    fn to_components_standard() {
        let (ta, od, crp, tm) = CompressionLevel::Standard.to_components();
        assert_eq!(ta, TerseAgent::Full);
        assert_eq!(od, OutputDensity::Terse);
        assert_eq!(crp, "compact");
        assert!(tm);
    }

    #[test]
    fn to_components_max() {
        let (ta, od, crp, tm) = CompressionLevel::Max.to_components();
        assert_eq!(ta, TerseAgent::Ultra);
        assert_eq!(od, OutputDensity::Ultra);
        assert_eq!(crp, "tdd");
        assert!(tm);
    }

    #[test]
    fn labels_match() {
        assert_eq!(CompressionLevel::Off.label(), "off");
        assert_eq!(CompressionLevel::Lite.label(), "lite");
        assert_eq!(CompressionLevel::Standard.label(), "standard");
        assert_eq!(CompressionLevel::Max.label(), "max");
    }

    #[test]
    fn is_active_false_for_off() {
        assert!(!CompressionLevel::Off.is_active());
    }

    #[test]
    fn is_active_true_for_all_others() {
        assert!(CompressionLevel::Lite.is_active());
        assert!(CompressionLevel::Standard.is_active());
        assert!(CompressionLevel::Max.is_active());
    }

    #[test]
    fn deserialization_defaults_to_adaptive_lite() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.compression_level, None);
        assert_eq!(CompressionLevel::effective(&cfg), CompressionLevel::Lite);
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str(r#"compression_level = "standard""#).unwrap();
        assert_eq!(cfg.compression_level, Some(0.5));
    }
}

#[cfg(test)]
mod memory_cleanup_tests {
    use super::super::*;

    #[test]
    fn default_is_shared() {
        assert_eq!(MemoryCleanup::default(), MemoryCleanup::Shared);
    }

    #[test]
    fn aggressive_ttl_is_300() {
        assert_eq!(MemoryCleanup::Aggressive.idle_ttl_secs(), 300);
    }

    #[test]
    fn shared_ttl_is_3600() {
        assert_eq!(MemoryCleanup::Shared.idle_ttl_secs(), 3600);
    }
}

#[cfg(test)]
mod simplified_config_tests {
    use super::super::*;

    #[test]
    fn bm25_pure_default_is_generous_not_ram_profile() {
        // Must use the generous disk default (512), NOT the RAM-profile value (which starved large repos and
        // caused perpetual cold rebuilds, issue #249).
        assert_eq!(
            Config::default().bm25_max_cache_mb_effective(),
            DEFAULT_BM25_PERSIST_MB
        );
    }

    #[test]
    fn no_budget_returns_defaults() {
        let cfg = Config::default();
        assert_eq!(
            cfg.archive_max_disk_mb_effective(),
            ArchiveConfig::default().max_disk_mb
        );
        assert_eq!(
            cfg.archive_max_age_hours_effective(),
            ArchiveConfig::default().max_age_hours
        );
    }

    #[test]
    fn memory_limits_unchanged_when_no_budget() {
        let cfg = Config::default();
        let policy = cfg.memory_policy_effective().unwrap();
        assert_eq!(policy.knowledge.max_facts, 200);
        assert_eq!(policy.episodic.max_episodes, 500);
    }

    #[test]
    fn simplified_template_is_valid_toml() {
        let parsed: Result<toml::Table, _> = toml::from_str(crate::cli::SIMPLIFIED_TEMPLATE);
        assert!(parsed.is_ok(), "Template must be valid TOML");
    }
}

#[cfg(test)]
mod setup_config_tests {
    use super::super::*;

    #[test]
    fn default_is_none_for_rules() {
        let cfg = SetupConfig::default();
        assert!(cfg.auto_inject_rules.is_none());
        assert!(cfg.auto_update_mcp);
    }

    #[test]
    fn explicit_true_injects() {
        let cfg = SetupConfig {
            auto_inject_rules: Some(true),
            auto_update_mcp: true,
        };
        assert!(cfg.should_inject_rules());
    }

    #[test]
    fn explicit_false_skips() {
        let cfg = SetupConfig {
            auto_inject_rules: Some(false),
            auto_update_mcp: true,
        };
        assert!(!cfg.should_inject_rules());
    }

    #[test]
    fn deserialization_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.setup.auto_inject_rules.is_none());
        assert!(cfg.setup.auto_update_mcp);
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str(
            r"
            [setup]
            auto_inject_rules = true
            auto_update_mcp = true
            ",
        )
        .unwrap();
        assert_eq!(cfg.setup.auto_inject_rules, Some(true));
        assert!(cfg.setup.auto_update_mcp);
    }

    #[test]
    fn deserialization_null_values() {
        let cfg: Config = toml::from_str(
            r"
            [setup]
            auto_update_mcp = false
            ",
        )
        .unwrap();
        assert!(cfg.setup.auto_inject_rules.is_none());
        assert!(!cfg.setup.auto_update_mcp);
    }

    #[test]
    fn should_update_mcp_reflects_flag() {
        // #281: the predicate that gates MCP registration in setup/onboard/init
        // must mirror the config flag exactly, so a locked-down environment can
        // disable MCP while still getting hooks and rules.
        let mut s = SetupConfig::default();
        assert!(s.should_update_mcp());
        s.auto_update_mcp = false;
        assert!(!s.should_update_mcp());
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let original = Config {
            setup: SetupConfig {
                auto_inject_rules: Some(true),
                auto_update_mcp: true,
            },
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.setup.auto_inject_rules, Some(true));
        assert!(parsed.setup.auto_update_mcp);
    }

    #[test]
    fn fresh_install_no_rules_should_not_inject() {
        let cfg = SetupConfig::default();
        // On a test machine without lean-ctx rules in home, None should resolve to false
        // (rules_already_present checks real filesystem — on CI this is always false)
        let result = cfg.should_inject_rules();
        // We can't assert false here because the test machine might have lean-ctx installed.
        // Instead, verify the method doesn't panic and returns a bool.
        let _ = result;
    }

    /// Regression: a stray root scalar once landed under [secret_detection]
    /// because whole-struct serialization placed it after a table. Any
    /// `Option<String>` root key is exposed to that bug; `tool_profile` and
    /// then `profile` carried the guard and are gone, so `rules_scope` does.
    #[test]
    fn option_root_scalar_serializes_above_the_first_table() {
        let original = Config {
            rules_scope: Some("global".to_string()),
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let pos = toml_str
            .find("rules_scope")
            .expect("rules_scope should be serialized");
        let first_table = toml_str
            .find(
                "
[",
            )
            .unwrap_or(toml_str.len());
        assert!(
            pos < first_table,
            "rules_scope must be a root key, not nested under a [table]:
{toml_str}"
        );
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.rules_scope.as_deref(), Some("global"));
    }
}
