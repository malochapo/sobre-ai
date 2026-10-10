//! Unit tests for [`Config`] parsing, defaults, and section behaviour.
//!
//! Extracted verbatim from `config/mod.rs` to keep that module focused.
//! These modules live at `config::tests::*`, so `super::super::*` resolves
//! to the `config` module (identical to the original `super::*`).

#[cfg(test)]
mod config_load_cache_tests {
    use super::super::*;
    use crate::core::data_dir::isolated_data_dir;

    /// Regression: `Config::load` must reflect a content change even when
    /// the file mtime is unchanged. A mtime-only cache (the old behaviour) kept a
    /// long-lived MCP server on a stale `search_roots` while a fresh `doctor`
    /// process — with an empty cache — saw the new value. The cache is now keyed
    /// on a content hash, so this scenario stays live.
    #[test]
    fn load_honors_content_change_with_preserved_mtime() {
        let _iso = isolated_data_dir();
        let path = Config::path().expect("config path under isolated data dir");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        // Warm the cache with no search roots (default).
        std::fs::write(&path, "# initial\n").unwrap();
        let mtime0 = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(Config::load().search_roots.is_empty());

        // Add a root but restore the original mtime, so any mtime-only
        // cache would serve the stale value.
        std::fs::write(&path, "search_roots = [\"/srv\"]\n").unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime0)).unwrap();

        assert!(
            Config::load().search_roots == ["/srv"],
            "Config::load() must honor a content change with unchanged mtime"
        );
    }
}

#[cfg(test)]
mod persist_global_tests {
    //! Regression tests: persisting config must never reset customized
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
        std::fs::write(&path, "compression_level = 0.5\n[proxy]\nport = 4444\n").unwrap();

        let returned = Config::update_global_at(&path, |c| c.proxy.enabled = Some(true))
            .expect("update_global_at must succeed");
        assert_eq!(returned.proxy.enabled, Some(true));

        let reloaded: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            reloaded.proxy.port,
            Some(4444),
            "customized value must survive"
        );
        assert_eq!(reloaded.compression_level, 0.5);
        assert_eq!(reloaded.proxy.enabled, Some(true));
    }

    // load_global reads only the given file.
    #[test]
    fn load_global_from_reads_only_the_given_file() {
        let (_dir, path) = tmp_config();
        std::fs::write(&path, "debug_log = true\n").unwrap();
        let cfg = Config::load_global_from(&path);
        assert!(cfg.debug_log);
    }

    // An unparseable config is refused and never clobbered.
    #[test]
    fn update_global_at_refuses_unparseable_and_leaves_file_untouched() {
        let (_dir, path) = tmp_config();
        let corrupt = "[proxy]\nport = = =\n";
        std::fs::write(&path, corrupt).unwrap();

        let result = Config::update_global_at(&path, |c| c.proxy.enabled = Some(true));
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
    fn update_global_at_refuses_an_unreadable_file_and_leaves_it_untouched() {
        let (_dir, path) = tmp_config();
        let latin1 = b"# caf\xe9\ncompression_level = 0.5\n";
        std::fs::write(&path, latin1).unwrap();

        let result = Config::update_global_at(&path, |c| c.proxy.enabled = Some(true));
        assert!(result.is_err(), "must refuse to modify a file it cannot read");
        assert_eq!(std::fs::read(&path).unwrap(), latin1);
    }

    #[test]
    fn load_global_from_missing_or_empty_yields_defaults() {
        let (_dir, path) = tmp_config();
        // Missing file.
        let cfg = Config::load_global_from(&path);
        assert_eq!(cfg.proxy.port, Config::default().proxy.port);
        assert_eq!(cfg.shell_timeout_secs, Config::default().shell_timeout_secs);
        // Empty / whitespace-only file.
        std::fs::write(&path, "   \n").unwrap();
        let cfg2 = Config::load_global_from(&path);
        assert_eq!(cfg2.proxy.port, Config::default().proxy.port);
        assert_eq!(
            cfg2.shell_timeout_secs,
            Config::default().shell_timeout_secs
        );
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

    // `missing_config_path` is the visible signal that the runtime
    // resolved a `config.toml` that doesn't exist — i.e. an edit landed in a
    // different file (a custom config dir, or a sandboxed HOME). The block messages
    // turn this into an over-MCP-visible note.
    #[test]
    fn missing_config_path_flags_absent_then_clears_when_present() {
        let _lock = crate::core::data_dir::test_env_lock();
        let saved = std::env::var("SOBRE_CONFIG_DIR").ok();
        let dir = tempfile::tempdir().unwrap();
        crate::test_env::set_var("SOBRE_CONFIG_DIR", dir.path());

        // No config.toml in the resolved dir → flagged as missing (on defaults).
        assert_eq!(
            Config::missing_config_path(),
            Some(dir.path().join("config.toml"))
        );

        // Create the file → the runtime now has a real config, so no flag.
        std::fs::write(dir.path().join("config.toml"), "ultra_compact = true\n").unwrap();
        assert!(Config::missing_config_path().is_none());

        crate::test_env::remove_var("SOBRE_CONFIG_DIR");
        if let Some(v) = saved {
            crate::test_env::set_var("SOBRE_CONFIG_DIR", v);
        }
    }
}
