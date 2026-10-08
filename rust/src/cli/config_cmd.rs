use crate::core::config;

pub fn cmd_config(args: &[String]) {
    match args.first().map(String::as_str) {
        None | Some("show") => {
            crate::outln!("{}", config::Config::load().show());
            crate::outln!(
                "\nChange a key with `sobre config set <key> <value>`; `sobre config init`\nwrites the file with every key documented. `sobre doctor` flags unknown keys."
            );
        }
        Some("init") => init_config(),
        Some("set") => {
            if args.len() < 3 {
                crate::errln!("Usage: sobre config set <key> <value>");
                std::process::exit(1);
            }
            let key = &args[1];
            let val = &args[2];

            // Upstream URLs are normalized before the single governed write
            // path below, so the #852 review covers every route.
            let (write_key, write_val): (String, String) = match key.as_str() {
                k if k.starts_with("proxy.upstreams.") => {
                    let effective = normalize_optional_upstream(val).unwrap_or_default();
                    (key.clone(), effective)
                }
                _ => (key.clone(), val.clone()),
            };

            write_config_key(&write_key, &write_val, key, val, args);
        }
        Some("path") => cmd_config_path(),
        _ => {
            crate::errln!("Usage: sobre config [show|set <key> <value>|path|init]");
            std::process::exit(1);
        }
    }
}

/// Single governed write path for `config set` (#852).
///
/// `key`/`value` are the resolved pair actually persisted; `display_key`/
/// `display_val` are what the user typed (kept for messaging, e.g. when a value
/// was normalized). Behavior:
/// - **no-op** (current == new): report "unchanged", write nothing.
/// - **consequential key** ([`config::risk`]): print a before→after review + a
///   risk note, then require confirmation or `--yes`; abort if declined.
/// - **routine key**: write directly, as before.
fn write_config_key(key: &str, value: &str, display_key: &str, display_val: &str, args: &[String]) {
    const BOLD: &str = "\x1b[1m";
    const DIM: &str = "\x1b[2m";
    const YELLOW: &str = "\x1b[33m";
    const RST: &str = "\x1b[0m";

    let current = config::setter::current_value(key);

    if current.as_deref() == Some(value) {
        crate::outln!("{display_key} is already set to {display_val} — unchanged.");
        return;
    }

    if let Some(risk) = config::risk::classify(key) {
        let before = current.as_deref().unwrap_or("(default)");
        let after = if value.is_empty() { "(default)" } else { value };
        crate::outln!("{BOLD}Review change to {display_key}{RST}");
        crate::outln!("  {before}  →  {after}");
        crate::outln!("  {YELLOW}{}{RST}", risk.note);
        if !super::prompt::confirm(
            &format!("Apply {display_key} = {display_val}?"),
            super::prompt::wants_yes(args),
        ) {
            crate::outln!("{DIM}Aborted — {display_key} left unchanged.{RST}");
            return;
        }
    }

    match config::setter::set_by_key(key, value) {
        Ok(_) => crate::outln!("Updated {display_key} = {display_val}"),
        Err(e) => {
            crate::errln!("{e}");
            std::process::exit(1);
        }
    }
}

/// Resolves the [`config::Config`] that `config init` should persist.
///
/// `existing_raw` is the verbatim content of the current `config.toml`. When it holds a non-empty, parseable document we
/// return its deserialized form so every user value is retained; an empty/absent
/// file yields defaults, and an unparseable one is surfaced as an error so the
/// caller can refuse to clobber it.
///
/// This is the regression guard for #443: `config init` once wrote
/// `Config::default()`, and `Config::save()` overwrites any key present in both
/// the incoming document and the file (see `config_io::merge_table`), which
/// silently reset customized values like `search_roots` or `compression_level`.
fn config_for_init(existing_raw: Option<&str>) -> Result<config::Config, String> {
    match existing_raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        Some(raw) => toml::from_str::<config::Config>(raw).map_err(|e| e.to_string()),
        None => Ok(config::Config::default()),
    }
}

/// Implements `config init`: (re)writes the global config as a fully
/// annotated reference document, seeded with the user's existing values (#443).
///
/// Unlike `save()` (which keeps the file minimal), this emits every key with its
/// documentation. The body is a verbatim serialization of the resolved config,
/// so no customized value is ever lost; an unparseable existing file is refused
/// upstream by [`config_for_init`] rather than clobbered.
fn init_config() {
    let Some(path) = config::Config::path() else {
        crate::errln!("Error: cannot determine the config path");
        return;
    };

    let existing_raw = std::fs::read_to_string(&path).ok();

    let cfg = match config_for_init(existing_raw.as_deref()) {
        Ok(cfg) => cfg,
        Err(e) => {
            crate::errln!(
                "Error: refusing to overwrite an unparseable config.toml ({e}).\n  \
                 Fix it manually or run `sobre doctor --fix`, then retry."
            );
            return;
        }
    };

    let schema = config::schema::ConfigSchema::generate();
    let rendered = config::render_annotated_config(&cfg, &schema);

    match crate::config_io::write_atomic_with_backup(&path, &rendered) {
        Ok(()) => crate::outln!("Wrote annotated config at {}", path.display()),
        Err(e) => crate::errln!("Error: {e}"),
    }
}

/// Print the absolute path of the resolved global `config.toml` (one line,
/// scriptable).
///
/// This is the canonical way to verify that the CLI and the MCP server resolve
/// the *same* config file (GH #594): run `sobre config path` in a terminal
/// and from the editor's MCP context, then compare. Output is the bare path so
/// it can be diffed/captured in scripts and tests; it exits non-zero only when
/// no config directory can be resolved at all.
fn cmd_config_path() {
    let Some(p) = config::Config::path() else {
        crate::errln!("Error: cannot resolve a config directory");
        std::process::exit(1);
    };
    crate::outln!("{}", p.display());
}

fn normalize_optional_upstream(value: &str) -> Option<String> {
    use crate::core::config::normalize_url_opt;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        None
    } else {
        normalize_url_opt(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reproduces `Config::save()`'s on-disk merge without touching the real
    // config path: serialize `cfg`, then merge it onto `existing` exactly as
    // save() does, and return the value that `shell_timeout_secs` ends up with.
    fn merged_shell_timeout(cfg: &config::Config, existing: &str) -> Option<u64> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, existing).unwrap();
        let new_content = toml::to_string_pretty(cfg).unwrap();
        let baseline = toml::from_str::<config::Config>("").unwrap();
        let defaults = toml::to_string_pretty(&baseline).unwrap();
        crate::config_io::write_toml_preserving_minimal(&path, &new_content, &defaults).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        toml::from_str::<config::Config>(&written)
            .unwrap()
            .shell_timeout_secs
    }

    #[test]
    fn init_uses_existing_values_not_defaults() {
        let existing = "shell_timeout_secs = 90\ncompression_level = 0.5\n";
        let cfg = config_for_init(Some(existing)).expect("parse existing");
        assert_eq!(
            cfg.shell_timeout_secs,
            Some(90),
            "must keep the user's value"
        );
        assert_eq!(cfg.compression_level, 0.5);
    }

    #[test]
    fn init_falls_back_to_defaults_on_fresh_install() {
        let cfg = config_for_init(None).expect("default");
        assert_eq!(
            cfg.shell_timeout_secs,
            config::Config::default().shell_timeout_secs
        );
        assert_eq!(
            cfg.compression_level,
            config::Config::default().compression_level
        );
        let cfg_empty = config_for_init(Some("   \n")).expect("blank -> default");
        assert_eq!(
            cfg_empty.shell_timeout_secs,
            config::Config::default().shell_timeout_secs
        );
    }

    #[test]
    fn init_refuses_unparseable_config() {
        assert!(config_for_init(Some("shell_timeout_secs = = =")).is_err());
    }

    // #443 end-to-end: `config init` must not reset a customized value.
    #[test]
    fn init_preserves_value_through_save_merge() {
        let existing = "shell_timeout_secs = 90\n";
        let cfg = config_for_init(Some(existing)).unwrap();
        assert_eq!(
            merged_shell_timeout(&cfg, existing),
            Some(90),
            "user value must survive `config init`"
        );
    }
}
