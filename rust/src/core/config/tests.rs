//! Unit tests for [`Config`] parsing, defaults, and section behaviour.
//!
//! Extracted verbatim from `config/mod.rs` to keep that module focused.
//! These modules live at `config::tests::*`, so `super::super::*` resolves
//! to the `config` module (identical to the original `super::*`).

#[cfg(test)]
mod no_degrade_tests {
    use super::super::*;

    #[test]
    fn is_off() {
        let cfg = Config::default();
        assert!(!cfg.no_degrade_effective());
    }
}

#[cfg(test)]
mod delta_explicit_tests {
    use super::super::*;

    #[test]
    fn is_on() {
        let cfg = Config::default();
        assert!(cfg.delta_explicit_effective());
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
}

#[cfg(test)]
mod config_load_cache_tests {
    use super::super::*;
    use crate::core::data_dir::isolated_data_dir;

    /// #406 regression: `Config::load()` must reflect a content change even when
    /// the file mtime is unchanged. A mtime-only cache (the old behaviour) kept a
    /// long-lived MCP server on a stale `allow_paths` while a fresh `doctor`
    /// process — with an empty cache — saw the new value. The cache is now keyed
    /// on a content hash, so this scenario stays live.
    #[test]
    fn load_honors_content_change_with_preserved_mtime() {
        let _iso = isolated_data_dir();
        let path = Config::path().expect("config path under isolated data dir");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        // Warm the cache with the jail closed (default).
        std::fs::write(&path, "# initial\n").unwrap();
        let mtime0 = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(!Config::load().jail_open());

        // Open the jail but restore the original mtime, so any mtime-only
        // cache would serve the stale value (#406).
        std::fs::write(&path, "allow_paths = [\"/\"]\n").unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime0)).unwrap();

        assert!(
            Config::load().jail_open(),
            "Config::load() must honor a content change with unchanged mtime (#406)"
        );
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
        std::fs::write(&path, "proxy_port = 4444\ncompression_level = 0.5\n").unwrap();

        let returned = Config::update_global_at(&path, |c| c.proxy_enabled = Some(true))
            .expect("update_global_at must succeed");
        assert_eq!(returned.proxy_enabled, Some(true));

        let reloaded: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            reloaded.proxy_port,
            Some(4444),
            "customized value must survive"
        );
        assert_eq!(reloaded.compression_level, Some(0.5));
        assert_eq!(reloaded.proxy_enabled, Some(true));
    }

    // load_global reads only the given file.
    #[test]
    fn load_global_from_reads_only_the_given_file() {
        let (_dir, path) = tmp_config();
        std::fs::write(&path, "rules_scope = \"global\"\n").unwrap();
        let cfg = Config::load_global_from(&path);
        assert_eq!(cfg.rules_scope.as_deref(), Some("global"));
    }

    // Subticket 4 contract: refuse to touch an unparseable config; never clobber.
    #[test]
    fn update_global_at_refuses_unparseable_and_leaves_file_untouched() {
        let (_dir, path) = tmp_config();
        let corrupt = "proxy_port = = =\n";
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
        assert_eq!(cfg.proxy_port, Config::default().proxy_port);
        assert_eq!(cfg.shell_timeout_secs, Config::default().shell_timeout_secs);
        // Empty / whitespace-only file.
        std::fs::write(&path, "   \n").unwrap();
        let cfg2 = Config::load_global_from(&path);
        assert_eq!(cfg2.proxy_port, Config::default().proxy_port);
        assert_eq!(
            cfg2.shell_timeout_secs,
            Config::default().shell_timeout_secs
        );
    }
}

#[cfg(test)]
mod max_index_threads_tests {
    use super::super::*;

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
    }

    #[test]
    fn timeout_overrides_parse_from_toml() {
        let cfg: Config = toml::from_str("shell_timeout_secs = 90").unwrap();
        assert_eq!(cfg.shell_timeout_secs, Some(90));
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
        let cfg = Config::default();
        assert_eq!(cfg.context_budget_tokens_effective(), 8000);
        assert!(cfg.proactive_expansion_effective());
        assert_eq!(cfg.proactive_expansion_budget_tokens_effective(), 2000);
        assert_eq!(cfg.proactive_expansion_max_age_secs_effective(), 3600);
    }
}
