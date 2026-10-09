use crate::core::config;

pub fn cmd_config(args: &[String]) {
    match args.first().map(String::as_str) {
        None | Some("show") => {
            crate::outln!("{}", show());
            crate::outln!(
                "\nChange a key with `sobre config set <key> <value>`; `sobre config init`\nwrites the file with every key documented. `sobre doctor` flags unknown keys."
            );
        }
        Some("init") => init_config(),
        Some("set") => {
            if args.len() < 3 {
                crate::errln!("Usage: sobre config set <key> <value>");
                std::process::exit(2);
            }
            write_config_key(&args[1], &args[2], args);
        }
        Some("path") => cmd_config_path(),
        _ => {
            crate::errln!("Usage: sobre config [show|set <key> <value>|path|init]");
            std::process::exit(2);
        }
    }
}

/// Single governed write path for `config set`:
/// - an unknown key is refused first, with the keys `config set` takes;
/// - **no-op** (current == new): report "unchanged", write nothing;
/// - **consequential key** ([`config::risk`]): print a before→after review + a
///   risk note, then require confirmation or `--yes`; abort if declined;
/// - **routine key**: write directly.
///
/// `default` (or an empty value) removes the key, as does the key's default
/// value: the documented default then applies.
fn write_config_key(key: &str, value: &str, args: &[String]) {
    const BOLD: &str = "\x1b[1m";
    const DIM: &str = "\x1b[2m";
    const YELLOW: &str = "\x1b[33m";
    const RST: &str = "\x1b[0m";

    let keys = config::setter::settable_keys();
    if !keys.iter().any(|k| k == key) {
        crate::errln!("Unknown config key: {key}");
        if key == "proxy.upstreams" {
            crate::errln!(
                "  it is a table: set proxy.upstreams.anthropic or proxy.upstreams.openai"
            );
        } else if let Some(near) =
            crate::core::levenshtein::closest(key, keys.iter().map(String::as_str))
        {
            crate::errln!("  did you mean '{near}'?");
        }
        crate::errln!("  keys: {}", keys.join(", "));
        std::process::exit(2);
    }

    // Checked before any review: a value the loader would drop is refused.
    let checked = match config::setter::checked_value(key, value) {
        Ok(checked) => checked,
        Err(e) => {
            crate::errln!("{e}");
            std::process::exit(2);
        }
    };
    let unset = checked.is_none();
    let current = config::setter::current_value(key);
    if current.as_deref() == Some(value) || (unset && current.is_none()) {
        crate::outln!(
            "{key} is already {} — unchanged.",
            current.as_deref().unwrap_or("at its default")
        );
        return;
    }

    if let Some(risk) = config::risk::classify(key) {
        let before = current.as_deref().unwrap_or("(default)");
        let after = if unset { "(default)" } else { value };
        crate::outln!("{BOLD}Review change to {key}{RST}");
        crate::outln!("  {before}  →  {after}");
        crate::outln!("  {YELLOW}{}{RST}", risk.note);
        if !super::prompt::confirm(
            &format!("Apply {key} = {after}?"),
            super::prompt::wants_yes(args),
        ) {
            crate::errln!("{DIM}Aborted — {key} left unchanged.{RST}");
            std::process::exit(1);
        }
    }

    match config::setter::set_by_key(key, value) {
        Ok(_) => match config::setter::current_value(key) {
            Some(stored) => crate::outln!("Updated {key} = {stored}"),
            None => crate::outln!("Removed {key}: its default applies."),
        },
        Err(e) => {
            crate::errln!("{e}");
            std::process::exit(2);
        }
    }
}

/// `sobre config`: every key `config set` takes, with the value in effect —
/// the file's, or the documented default.
fn show() -> String {
    let path = config::Config::path().map_or_else(
        || "(config dir unresolved)".to_string(),
        |p| p.display().to_string(),
    );
    let schema = config::schema::ConfigSchema::generate();
    let mut out = format!("Config: {path}\n");
    // Root keys first: a `[table]` header ends them.
    let sections = schema
        .sections
        .iter()
        .filter(|(s, _)| s.as_str() == "root")
        .chain(schema.sections.iter().filter(|(s, _)| s.as_str() != "root"));
    for (section, section_schema) in sections {
        out.push('\n');
        if section != "root" {
            out.push_str(&format!("[{section}]\n"));
        }
        for (field, key_schema) in &section_schema.keys {
            let key = if section == "root" {
                field.clone()
            } else {
                format!("{section}.{field}")
            };
            let line = match config::setter::current_value(&key) {
                Some(value) => format!("{field} = {value}"),
                None if key_schema.default.is_null() => format!("{field}: not set"),
                None => format!("{field} = {}  (default)", key_schema.default),
            };
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Resolves the [`config::Config`] that `config init` should persist.
///
/// `existing_raw` is the verbatim content of the current `config.toml`. When it holds a non-empty, parseable document we
/// return its deserialized form so every user value is retained; an empty/absent
/// file yields defaults, and an unparseable one is surfaced as an error so the
/// caller can refuse to clobber it.
///
/// This is the regression guard: `config init` once wrote
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
/// annotated reference document, seeded with the user's existing values.
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
                 Fix it by hand, then retry."
            );
            std::process::exit(1);
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
/// the *same* config file: run `sobre config path` in a terminal
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

    // End-to-end: `config init` must not reset a customized value.
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
