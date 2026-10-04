//! Unit tests for [`Config`] parsing, defaults, and section behaviour.
//!
//! Extracted verbatim from `config/mod.rs` to keep that module focused.
//! These modules live at `config::tests::*`, so `super::super::*` resolves
//! to the `config` module (identical to the original `super::*`).

#[cfg(test)]
mod prefer_native_editor_tests {
    use super::super::*;

    #[test]
    fn default_is_off_and_blocks_nothing() {
        let cfg = Config::default();
        assert!(!cfg.prefer_native_editor);
        assert!(!cfg.prefer_native_editor_effective());
        assert!(!cfg.edit_tool_blocked("ctx_patch"));
    }

    #[test]
    fn enabled_blocks_only_edit_tools() {
        let cfg = Config {
            prefer_native_editor: true,
            ..Default::default()
        };
        // #454: the tools that edit source files are blocked; reads/search stay available.
        assert!(cfg.edit_tool_blocked("ctx_patch"));
        assert!(cfg.edit_tool_blocked("ctx_refactor"));
        assert!(!cfg.edit_tool_blocked("ctx_read"));
        assert!(!cfg.edit_tool_blocked("ctx_search"));
        assert!(!cfg.edit_tool_blocked("ctx_callgraph"));
    }

    /// Every name in `EDIT_TOOL_NAMES` must actually be blocked — the list and
    /// the predicate must not drift apart.
    #[test]
    fn every_listed_edit_tool_is_blocked() {
        let cfg = Config {
            prefer_native_editor: true,
            ..Default::default()
        };
        for name in crate::core::config::EDIT_TOOL_NAMES {
            assert!(cfg.edit_tool_blocked(name), "{name} must be blocked");
        }
    }
}

#[cfg(test)]
mod no_degrade_tests {
    use super::super::*;

    // --- Defaults ---

    #[test]
    fn default_is_false() {
        let cfg = Config::default();
        assert!(!cfg.no_degrade);
    }

    #[test]
    fn effective_false_when_unset() {
        let cfg = Config::default();
        assert!(!cfg.no_degrade_effective());
    }

    // --- Config field ---

    #[test]
    fn config_field_true_respected() {
        let cfg = Config {
            no_degrade: true,
            ..Default::default()
        };
        assert!(cfg.no_degrade_effective());
    }

    // --- TOML deserialization ---

    #[test]
    fn deserialization_true() {
        let cfg: Config = toml::from_str("no_degrade = true").unwrap();
        assert!(cfg.no_degrade);
    }

    #[test]
    fn deserialization_false() {
        let cfg: Config = toml::from_str("no_degrade = false").unwrap();
        assert!(!cfg.no_degrade);
    }

    #[test]
    fn deserialization_absent_defaults_false() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.no_degrade);
    }

    // --- Coexistence with other config fields ---

    #[test]
    fn no_degrade_independent_of_prefer_native_editor() {
        let cfg = Config {
            no_degrade: true,
            prefer_native_editor: true,
            ..Default::default()
        };
        assert!(cfg.no_degrade_effective());
        assert!(cfg.prefer_native_editor);
    }
}

#[cfg(test)]
mod delta_explicit_tests {
    use super::super::*;

    // --- Defaults ---

    #[test]
    fn default_is_on() {
        // Flipped on 2026-08-27: a changed cached file comes back as a diff.
        let cfg = Config::default();
        assert!(cfg.delta_explicit);
    }

    #[test]
    fn effective_follows_the_default_when_unset() {
        let cfg = Config::default();
        assert!(cfg.delta_explicit_effective());
    }

    // --- Config field ---

    #[test]
    fn config_field_false_respected() {
        let cfg = Config {
            delta_explicit: false,
            ..Default::default()
        };
        assert!(!cfg.delta_explicit_effective());
    }

    // --- TOML deserialization ---

    #[test]
    fn deserialization_true() {
        let cfg: Config = toml::from_str("delta_explicit = true").unwrap();
        assert!(cfg.delta_explicit);
    }

    #[test]
    fn deserialization_false() {
        let cfg: Config = toml::from_str("delta_explicit = false").unwrap();
        assert!(!cfg.delta_explicit);
    }

    #[test]
    fn deserialization_absent_defaults_false() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.delta_explicit);
    }

    // --- Round-trip (serialize → deserialize preserves the field) ---

    #[test]
    fn round_trip_preserves_field() {
        let cfg = Config {
            delta_explicit: true,
            ..Default::default()
        };
        let serialized = toml::to_string(&cfg).expect("Config must serialize to TOML");
        let restored: Config =
            toml::from_str(&serialized).expect("serialized Config must round-trip");
        assert!(
            restored.delta_explicit,
            "delta_explicit must survive a TOML serialize → deserialize round-trip"
        );
    }

    // --- Coexistence with other config fields ---

    #[test]
    fn delta_explicit_independent_of_no_degrade() {
        let cfg = Config {
            delta_explicit: true,
            no_degrade: true,
            ..Default::default()
        };
        assert!(cfg.delta_explicit_effective());
        assert!(cfg.no_degrade_effective());
    }
}

#[cfg(test)]
mod rules_scope_tests {
    use super::super::*;

    #[test]
    fn default_is_both() {
        let cfg = Config::default();
        assert_eq!(cfg.rules_scope_effective(), RulesScope::Both);
    }

    #[test]
    fn config_global() {
        let cfg = Config {
            rules_scope: Some("global".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.rules_scope_effective(), RulesScope::Global);
    }

    #[test]
    fn unknown_value_falls_back_to_both() {
        let cfg = Config {
            rules_scope: Some("nonsense".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.rules_scope_effective(), RulesScope::Both);
    }

    #[test]
    fn deserialization_none_by_default() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.rules_scope.is_none());
        assert_eq!(cfg.rules_scope_effective(), RulesScope::Both);
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str(r#"rules_scope = "project""#).unwrap();
        assert_eq!(cfg.rules_scope.as_deref(), Some("project"));
        assert_eq!(cfg.rules_scope_effective(), RulesScope::Project);
    }
}

#[cfg(test)]
mod rules_injection_tests {
    use super::super::*;

    #[test]
    fn config_off() {
        for raw in ["off", "none", "disabled"] {
            let cfg = Config {
                rules_injection: Some(raw.to_string()),
                ..Default::default()
            };
            assert_eq!(
                cfg.rules_injection_effective(),
                RulesInjection::Off,
                "{raw:?} should resolve to Off"
            );
        }
    }

    #[test]
    fn off_disables_dedicated_session_context() {
        let cfg = Config {
            rules_injection: Some("off".to_string()),
            ..Default::default()
        };
        assert!(!cfg.dedicated_session_context_active());
    }

    #[test]
    fn unknown_value_falls_back_to_shared() {
        let cfg = Config {
            rules_injection: Some("nonsense".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.rules_injection_effective(), RulesInjection::Shared);
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str(r#"rules_injection = "dedicated""#).unwrap();
        assert_eq!(cfg.rules_injection.as_deref(), Some("dedicated"));
        assert_eq!(cfg.rules_injection_effective(), RulesInjection::Dedicated);
    }

    #[test]
    fn dedicated_session_context_gated_by_scope() {
        // Dedicated + non-project scope → SessionStart summary active.
        let cfg = Config {
            rules_injection: Some("dedicated".to_string()),
            ..Default::default()
        };
        assert!(cfg.dedicated_session_context_active());

        // Dedicated + project scope → global summary suppressed (project files only).
        let cfg = Config {
            rules_injection: Some("dedicated".to_string()),
            rules_scope: Some("project".to_string()),
            ..Default::default()
        };
        assert!(!cfg.dedicated_session_context_active());

        // Shared (default) → never the SessionStart summary path.
        let cfg = Config::default();
        assert!(!cfg.dedicated_session_context_active());
    }
}

#[cfg(test)]
mod permission_inheritance_tests {
    use super::super::*;

    #[test]
    fn default_is_off() {
        let cfg = Config::default();
        assert_eq!(
            cfg.permission_inheritance_effective(),
            PermissionInheritance::Off
        );
    }

    #[test]
    fn config_on() {
        let cfg = Config {
            permission_inheritance: Some("on".to_string()),
            ..Default::default()
        };
        assert_eq!(
            cfg.permission_inheritance_effective(),
            PermissionInheritance::On
        );
    }

    #[test]
    fn unknown_value_falls_back_to_off() {
        let cfg = Config {
            permission_inheritance: Some("nonsense".to_string()),
            ..Default::default()
        };
        assert_eq!(
            cfg.permission_inheritance_effective(),
            PermissionInheritance::Off
        );
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str(r#"permission_inheritance = "on""#).unwrap();
        assert_eq!(cfg.permission_inheritance.as_deref(), Some("on"));
    }
}

#[cfg(test)]
mod loop_detection_config_tests {
    use super::super::*;

    #[test]
    fn defaults_are_reasonable() {
        let cfg = LoopDetectionConfig::default();
        assert_eq!(cfg.normal_threshold, 2);
        assert_eq!(cfg.reduced_threshold, 4);
        // 0 = blocking disabled by default (LeanCTX philosophy: always help, never block)
        assert_eq!(cfg.blocked_threshold, 0);
        assert_eq!(cfg.window_secs, 300);
        assert_eq!(cfg.search_group_limit, 10);
    }

    #[test]
    fn deserialization_defaults_when_missing() {
        let cfg: Config = toml::from_str("").unwrap();
        // 0 = blocking disabled by default
        assert_eq!(cfg.loop_detection.blocked_threshold, 0);
        assert_eq!(cfg.loop_detection.search_group_limit, 10);
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str(
            r"
            [loop_detection]
            normal_threshold = 1
            reduced_threshold = 3
            blocked_threshold = 5
            window_secs = 120
            search_group_limit = 8
            ",
        )
        .unwrap();
        assert_eq!(cfg.loop_detection.normal_threshold, 1);
        assert_eq!(cfg.loop_detection.reduced_threshold, 3);
        assert_eq!(cfg.loop_detection.blocked_threshold, 5);
        assert_eq!(cfg.loop_detection.window_secs, 120);
        assert_eq!(cfg.loop_detection.search_group_limit, 8);
    }

    #[test]
    fn partial_override_keeps_defaults() {
        let cfg: Config = toml::from_str(
            r"
            [loop_detection]
            blocked_threshold = 10
            ",
        )
        .unwrap();
        assert_eq!(cfg.loop_detection.blocked_threshold, 10);
        assert_eq!(cfg.loop_detection.normal_threshold, 2);
        assert_eq!(cfg.loop_detection.search_group_limit, 10);
    }
}

#[cfg(test)]
mod extra_roots_tests {
    use super::super::*;

    #[test]
    fn default_is_empty() {
        let cfg = Config::default();
        assert!(cfg.extra_roots.is_empty());
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str(r#"extra_roots = ["/data/store", "/test/env"]"#).unwrap();
        assert_eq!(cfg.extra_roots, vec!["/data/store", "/test/env"]);
    }
}

#[cfg(test)]
mod config_load_cache_tests {
    use super::super::*;
    use crate::core::data_dir::isolated_data_dir;

    /// #406 regression: `Config::load()` must reflect a content change even when
    /// the file mtime is unchanged. A mtime-only cache (the old behaviour) kept a
    /// long-lived MCP server on a stale `path_jail` while a fresh `doctor`
    /// process — with an empty cache — saw the new value. The cache is now keyed
    /// on a content hash, so this scenario stays live.
    #[test]
    fn load_honors_content_change_with_preserved_mtime() {
        let _iso = isolated_data_dir();
        let path = Config::path().expect("config path under isolated data dir");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        // Warm the cache with jail unset (default None).
        std::fs::write(&path, "# initial\n").unwrap();
        let mtime0 = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(Config::load().path_jail, None);

        // Flip path_jail=false but restore the original mtime, so any mtime-only
        // cache would serve the stale value (#406).
        std::fs::write(&path, "path_jail = false\n").unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime0)).unwrap();

        assert_eq!(
            Config::load().path_jail,
            Some(false),
            "Config::load() must honor a content change with unchanged mtime (#406)"
        );
    }
}

#[cfg(test)]
mod cost_config_tests {
    use super::super::*;

    #[test]
    fn default_is_empty() {
        let cfg = CostConfig::default();
        assert!(cfg.default_model.is_none());
        assert!(cfg.models.is_empty());
        assert_eq!(cfg.model_for_client("cursor"), None);
    }

    #[test]
    fn per_client_overrides_default() {
        let mut models = std::collections::HashMap::new();
        models.insert("cursor".to_string(), "claude-opus-4.5".to_string());
        let cfg = CostConfig {
            default_model: Some("gpt-5.4".to_string()),
            models,
            ..Default::default()
        };
        assert_eq!(
            cfg.model_for_client("cursor").as_deref(),
            Some("claude-opus-4.5")
        );
        // No entry → global default.
        assert_eq!(cfg.model_for_client("copilot").as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn blank_values_are_ignored() {
        let cfg = CostConfig {
            default_model: Some("   ".to_string()),
            models: std::collections::HashMap::new(),
            ..Default::default()
        };
        assert_eq!(cfg.model_for_client("cursor"), None);
    }

    #[test]
    fn parses_from_toml_section() {
        let cfg: Config = toml::from_str(
            r#"
[cost]
default_model = "claude-opus-4.5"

[cost.models]
cursor = "claude-opus-4.5"
copilot = "gpt-5.4"
"#,
        )
        .unwrap();
        assert_eq!(cfg.cost.default_model.as_deref(), Some("claude-opus-4.5"));
        assert_eq!(
            cfg.cost.model_for_client("copilot").as_deref(),
            Some("gpt-5.4")
        );
    }

    #[test]
    fn default_config_has_empty_cost_section() {
        let cfg = Config::default();
        assert!(cfg.cost.default_model.is_none());
        assert!(cfg.cost.models.is_empty());
        assert!(cfg.cost.prices.is_empty());
    }

    #[test]
    fn parses_price_overrides_from_toml() {
        // #1189: negotiated enterprise rates in config.toml.
        let cfg: Config = toml::from_str(
            r#"
[cost.prices."internal-llm"]
input_per_m = 0.10
output_per_m = 0.40

[cost.prices."claude-opus-4.5"]
input_per_m = 4.0
"#,
        )
        .unwrap();
        let internal = cfg.cost.prices.get("internal-llm").expect("row parsed");
        assert_eq!(internal.input_per_m, Some(0.10));
        assert_eq!(internal.output_per_m, Some(0.40));
        assert_eq!(internal.cache_read_per_m, None);
        let opus = cfg.cost.prices.get("claude-opus-4.5").expect("row parsed");
        assert_eq!(opus.input_per_m, Some(4.0));
        assert_eq!(opus.output_per_m, None);
    }
}

#[cfg(test)]
mod persist_global_tests {
    //! Regression tests for #443: persisting config must never reset customized
    //! values.
    use super::super::*;

    fn tmp_config() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        (dir, path)
    }

    // The canonical persist path keeps every customized value and applies only
    // the requested change.
    #[test]
    fn update_global_at_preserves_customized_and_persists_change() {
        let (_dir, path) = tmp_config();
        std::fs::write(
            &path,
            "max_ram_percent = 30\ncompression_level = \"standard\"\n",
        )
        .unwrap();

        let returned = Config::update_global_at(&path, |c| c.proxy_enabled = Some(true))
            .expect("update_global_at must succeed");
        assert_eq!(returned.proxy_enabled, Some(true));

        let reloaded: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            reloaded.max_ram_percent, 30,
            "customized value must survive"
        );
        assert_eq!(reloaded.compression_level, Some(0.5));
        assert_eq!(reloaded.proxy_enabled, Some(true));
    }

    // load_global reads only the given file.
    #[test]
    fn load_global_from_reads_only_the_given_file() {
        let (_dir, path) = tmp_config();
        std::fs::write(&path, "theme = \"global-theme\"\n").unwrap();
        let cfg = Config::load_global_from(&path);
        assert_eq!(cfg.theme, "global-theme");
    }

    // Subticket 4 contract: refuse to touch an unparseable config; never clobber.
    #[test]
    fn update_global_at_refuses_unparseable_and_leaves_file_untouched() {
        let (_dir, path) = tmp_config();
        let corrupt = "max_ram_percent = = =\n";
        std::fs::write(&path, corrupt).unwrap();

        let result = Config::update_global_at(&path, |c| c.proxy_enabled = Some(true));
        assert!(
            result.is_err(),
            "must refuse to modify an unparseable config"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "the corrupt file must be left exactly as-is"
        );
    }

    #[test]
    fn load_global_from_missing_or_empty_yields_defaults() {
        let (_dir, path) = tmp_config();
        // Missing file.
        let cfg = Config::load_global_from(&path);
        assert_eq!(cfg.max_ram_percent, Config::default().max_ram_percent);
        // Empty / whitespace-only file.
        std::fs::write(&path, "   \n").unwrap();
        let cfg2 = Config::load_global_from(&path);
        assert_eq!(cfg2.max_ram_percent, Config::default().max_ram_percent);
    }
}

#[cfg(test)]
mod max_index_threads_tests {
    use super::super::*;

    #[test]
    fn default_is_zero() {
        assert_eq!(Config::default().max_index_threads, 0);
    }

    #[test]
    fn absent_in_toml_defaults_to_zero() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.max_index_threads, 0);
    }

    #[test]
    fn parses_explicit_cap_from_toml() {
        let cfg: Config = toml::from_str("max_index_threads = 8").unwrap();
        assert_eq!(cfg.max_index_threads, 8);
    }

    #[test]
    fn effective_uses_config() {
        let cfg = Config {
            max_index_threads: 4,
            ..Default::default()
        };
        assert_eq!(cfg.max_index_threads_effective(), 4);
    }

    #[test]
    fn effective_zero_means_no_cap() {
        assert_eq!(Config::default().max_index_threads_effective(), 0);
    }
}

#[cfg(test)]
mod shell_timeout_and_writes_tests {
    use super::super::*;

    #[test]
    fn timeout_overrides_default_to_none() {
        let cfg = Config::default();
        assert_eq!(cfg.shell_timeout_secs, None);
        assert_eq!(cfg.shell_heavy_timeout_secs, None);
    }

    #[test]
    fn timeout_overrides_parse_from_toml() {
        let cfg: Config =
            toml::from_str("shell_timeout_secs = 90\nshell_heavy_timeout_secs = 1800").unwrap();
        assert_eq!(cfg.shell_timeout_secs, Some(90));
        assert_eq!(cfg.shell_heavy_timeout_secs, Some(1800));
    }
}

#[cfg(test)]
mod config_path_visibility_tests {
    use super::super::*;

    // #540: `missing_config_path()` is the visible signal that the runtime
    // resolved a `config.toml` that doesn't exist — i.e. an edit landed in a
    // different file (XDG vs legacy dir, or a sandboxed HOME). The block messages
    // turn this into an over-MCP-visible note.
    #[test]
    fn missing_config_path_flags_absent_then_clears_when_present() {
        let _lock = crate::core::data_dir::test_env_lock();
        let saved = std::env::var("LEAN_CTX_CONFIG_DIR").ok();
        let dir = tempfile::tempdir().unwrap();
        crate::test_env::set_var("LEAN_CTX_CONFIG_DIR", dir.path());

        // No config.toml in the resolved dir → flagged as missing (on defaults).
        assert_eq!(
            Config::missing_config_path(),
            Some(dir.path().join("config.toml"))
        );

        // Create the file → the runtime now has a real config, so no flag.
        std::fs::write(dir.path().join("config.toml"), "ultra_compact = true\n").unwrap();
        assert!(Config::missing_config_path().is_none());

        crate::test_env::remove_var("LEAN_CTX_CONFIG_DIR");
        if let Some(v) = saved {
            crate::test_env::set_var("LEAN_CTX_CONFIG_DIR", v);
        }
    }
}

#[cfg(test)]
mod context_budget_tests {
    use super::super::*;

    #[test]
    fn default_budget_is_eight_thousand() {
        assert_eq!(ContextConfig::default().budget_tokens, 8000);
        assert_eq!(Config::default().context.budget_tokens, 8000);
        assert!(Config::default().context.proactive_expansion);
        assert_eq!(
            Config::default().context.proactive_expansion_budget_tokens,
            2000
        );
        assert_eq!(
            Config::default().context.proactive_expansion_max_age_secs,
            3600
        );
    }

    #[test]
    fn effective_uses_config_field() {
        let cfg = Config {
            context: ContextConfig {
                budget_tokens: 5000,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.context_budget_tokens_effective(), 5000);
    }

    #[test]
    fn proactive_expansion_settings_are_effective() {
        let cfg = Config {
            context: ContextConfig {
                proactive_expansion: false,
                proactive_expansion_budget_tokens: 17,
                proactive_expansion_threshold: 0.8,
                proactive_expansion_max_age_secs: 42,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!cfg.proactive_expansion_effective());
        assert_eq!(cfg.proactive_expansion_budget_tokens_effective(), 17);
        assert_eq!(cfg.proactive_expansion_threshold_effective(), 0.8);
        assert_eq!(cfg.proactive_expansion_max_age_secs_effective(), 42);
    }
}
