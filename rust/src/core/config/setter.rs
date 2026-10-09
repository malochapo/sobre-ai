//! Generic config setter that works for ALL schema-known keys.
//!
//! Instead of a hardcoded match arm per key, this module:
//! 1. Validates the key against `ConfigSchema`
//! 2. Parses the value according to the schema type
//! 3. Performs a TOML round-trip to set the value
//! 4. Deserializes back into `Config` for full serde validation

use super::Config;
use super::schema::{ConfigSchema, KeySchema};

/// Sets `key` to `value` in `config.toml`, editing the file in place so its
/// comments and layout stay:
///
/// - the value is parsed by the key's schema type and checked like the
///   config loader checks it (ranges, a port above 0, an `http(s)://`
///   upstream), so a value the loader would drop is refused here;
/// - `default`, an empty value, or the key's default value removes the key:
///   the file then says nothing and the documented default applies.
///
/// Returns the resulting `Config`.
pub fn set_by_key(key: &str, value: &str) -> Result<Config, crate::core::error::ConfigError> {
    use crate::core::error::ConfigError;
    let schema = ConfigSchema::generate();
    let key_schema = schema.lookup(key).ok_or_else(|| ConfigError::UnknownKey {
        key: key.to_string(),
    })?;
    let path = Config::path().ok_or(ConfigError::MissingPath)?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(ConfigError::Read { source }),
    };
    let mut doc = raw
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| ConfigError::InvalidValue {
            key: key.to_string(),
            message: format!("config.toml does not parse, fix it first: {e}"),
        })?;

    match checked_value(key, value)? {
        Some(toml_value) if !is_default(&toml_value, key_schema) => {
            set_key(&mut doc, key, &toml_value)?;
        }
        _ => remove_key(&mut doc, key),
    }

    let text = doc.to_string();
    let cfg: Config =
        toml::from_str(&text).map_err(|e: toml::de::Error| ConfigError::InvalidValue {
            key: key.to_string(),
            message: e.to_string(),
        })?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::config_io::write_atomic(&path, &text).map_err(|e| ConfigError::Save {
        source: Box::new(crate::core::error::SobreError::Config(e.into())),
    })?;
    Ok(cfg)
}

/// `value` parsed and checked for `key` as the config loader checks it, or
/// `None` for `default` and an empty value (the key is to be removed). An
/// upstream URL comes back normalized, as the proxy loads it.
pub fn checked_value(
    key: &str,
    value: &str,
) -> Result<Option<toml::Value>, crate::core::error::ConfigError> {
    use crate::core::error::ConfigError;
    let schema = ConfigSchema::generate();
    let key_schema = schema.lookup(key).ok_or_else(|| ConfigError::UnknownKey {
        key: key.to_string(),
    })?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    let toml_value = parse_value(trimmed, key_schema).map_err(|e| match e {
        ConfigError::InvalidValue { message, .. } => ConfigError::InvalidValue {
            key: key.to_string(),
            message,
        },
        other => other,
    })?;
    check_range(key, &toml_value)?;
    Ok(Some(match toml_value.as_str() {
        Some(url) if key.starts_with("proxy.upstreams.") => {
            toml::Value::String(super::proxy::validate_upstream_url(url).map_err(|message| {
                ConfigError::InvalidValue {
                    key: key.to_string(),
                    message,
                }
            })?)
        }
        _ => toml_value,
    }))
}

/// Whether `value` is what the key holds when the file says nothing.
fn is_default(value: &toml::Value, key_schema: &KeySchema) -> bool {
    !key_schema.default.is_null()
        && serde_json::to_value(value).is_ok_and(|v| v == key_schema.default)
}

/// Writes `value` at the dotted `key` (`proxy.upstreams.anthropic`).
fn set_key(
    doc: &mut toml_edit::DocumentMut,
    key: &str,
    value: &toml::Value,
) -> Result<(), crate::core::error::ConfigError> {
    let item = format!("v = {value}")
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|d| d.get("v").cloned())
        .ok_or_else(|| crate::core::error::ConfigError::InvalidValue {
            key: key.to_string(),
            message: format!("cannot write {value}"),
        })?;
    let parts: Vec<&str> = key.split('.').collect();
    let (parents, leaf) = parts.split_at(parts.len() - 1);
    let mut table = doc.as_table_mut();
    for part in parents {
        let entry = table.entry(part).or_insert_with(|| {
            // Implicit: no `[proxy]` header of its own, so nothing is left
            // behind once its keys go.
            let mut new = toml_edit::Table::new();
            new.set_implicit(true);
            toml_edit::Item::Table(new)
        });
        table = entry.as_table_mut().ok_or_else(|| {
            crate::core::error::ConfigError::NonTableParent {
                key: key.to_string(),
                part: (*part).to_string(),
            }
        })?;
    }
    match table
        .get_mut(leaf[0])
        .and_then(toml_edit::Item::as_value_mut)
    {
        // Keep the line's comments when the key is already there.
        Some(existing) => {
            let decor = existing.decor().clone();
            if let Ok(mut new_value) = item.into_value() {
                *new_value.decor_mut() = decor;
                *existing = new_value;
            }
        }
        None => {
            table.insert(leaf[0], item);
        }
    }
    Ok(())
}

/// Removes the dotted `key`, and a sub-table (`[proxy.upstreams]`) the
/// removal leaves empty. Comment lines written above the key are kept: they
/// move to the next key, or to the end of the file.
fn remove_key(doc: &mut toml_edit::DocumentMut, key: &str) {
    let parts: Vec<&str> = key.split('.').collect();
    let (parents, leaf) = parts.split_at(parts.len() - 1);
    let mut orphaned = None;
    {
        let mut table = doc.as_table_mut();
        for part in parents {
            let Some(next) = table.get_mut(part).and_then(toml_edit::Item::as_table_mut) else {
                return;
            };
            table = next;
        }
        let prefix = table
            .key(leaf[0])
            .and_then(|k| k.leaf_decor().prefix())
            .and_then(toml_edit::RawString::as_str)
            .filter(|p| p.lines().any(|l| l.trim_start().starts_with('#')))
            .map(str::to_string);
        let names: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
        if table.remove(leaf[0]).is_none() {
            return;
        }
        if let Some(prefix) = prefix {
            let next = names
                .iter()
                .skip_while(|n| n.as_str() != leaf[0])
                .nth(1)
                .filter(|n| table.get(n).is_some_and(|i| !i.is_table()));
            match next.and_then(|n| table.key_mut(n)) {
                Some(mut next_key) => {
                    let old = next_key
                        .leaf_decor()
                        .prefix()
                        .and_then(toml_edit::RawString::as_str)
                        .unwrap_or("")
                        .to_string();
                    next_key
                        .leaf_decor_mut()
                        .set_prefix(format!("{prefix}{old}"));
                }
                None => orphaned = Some(prefix),
            }
        }
    }
    if let Some(prefix) = orphaned {
        let trailing = doc.trailing().as_str().unwrap_or("").to_string();
        doc.set_trailing(format!("{trailing}{prefix}"));
    }
    if parents.len() >= 2 {
        let mut parent = doc.as_table_mut();
        for part in &parents[..parents.len() - 1] {
            match parent.get_mut(part).and_then(toml_edit::Item::as_table_mut) {
                Some(next) => parent = next,
                None => return,
            }
        }
        let last = parents[parents.len() - 1];
        if parent
            .get(last)
            .and_then(toml_edit::Item::as_table)
            .is_some_and(toml_edit::Table::is_empty)
        {
            parent.remove(last);
        }
    }
}

/// The keys `config set` takes, as dotted names: what the schema documents.
#[must_use]
pub fn settable_keys() -> Vec<String> {
    let schema = ConfigSchema::generate();
    let mut keys = Vec::new();
    let sections = schema
        .sections
        .iter()
        .filter(|(s, _)| s.as_str() == "root")
        .chain(schema.sections.iter().filter(|(s, _)| s.as_str() != "root"));
    for (section, section_schema) in sections {
        for field in section_schema.keys.keys() {
            keys.push(if section == "root" {
                field.clone()
            } else {
                format!("{section}.{field}")
            });
        }
    }
    keys
}

/// Returns the current value of `key` from the on-disk `config.toml`, rendered
/// as the string a user would type (no TOML quoting), or `None` if the key is
/// unset (still at its schema default).
///
/// Powers the before→after review for consequential `config set` writes.
/// Reads only the user's file, so an unset key is reported as `None` rather than
/// the default — the review then shows `(default) → <new>`.
#[must_use]
pub fn current_value(key: &str) -> Option<String> {
    let table = load_config_as_table().ok()?;
    let parts: Vec<&str> = key.split('.').collect();
    let (parents, leaf) = parts.split_at(parts.len() - 1);

    let mut current = &table;
    for part in parents {
        current = current.get(*part)?.as_table()?;
    }
    current.get(leaf[0]).map(display_toml_value)
}

/// Renders a scalar `toml::Value` the way a user types it on the CLI: strings
/// without quotes, arrays comma-joined, everything else via its TOML form.
fn display_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Array(items) => items
            .iter()
            .map(display_toml_value)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

fn load_config_as_table() -> Result<toml::Table, crate::core::error::ConfigError> {
    let path = Config::path().ok_or(crate::core::error::ConfigError::MissingPath)?;
    if !path.exists() {
        return Ok(toml::Table::new());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|source| crate::core::error::ConfigError::Read { source })?;
    raw.parse::<toml::Table>()
        .map_err(|source| crate::core::error::ConfigError::ParseToml { source })
}
fn parse_value(
    value: &str,
    schema: &KeySchema,
) -> Result<toml::Value, crate::core::error::ConfigError> {
    match schema.ty.as_str() {
        "bool" | "bool?" => match value {
            "true" | "1" | "yes" => Ok(toml::Value::Boolean(true)),
            "false" | "0" | "no" => Ok(toml::Value::Boolean(false)),
            _ => Err(crate::core::error::ConfigError::ExpectedBool {
                value: value.to_string(),
            }),
        },
        ty if ty.trim_end_matches('?').starts_with('u') => {
            let n: i64 =
                value
                    .parse()
                    .map_err(|_| crate::core::error::ConfigError::ExpectedInteger {
                        value: value.to_string(),
                    })?;
            if n < 0 {
                return Err(crate::core::error::ConfigError::ExpectedUnsignedInteger {
                    value: value.to_string(),
                });
            }
            let max = match ty.trim_end_matches('?') {
                "u8" => i64::from(u8::MAX),
                "u16" => i64::from(u16::MAX),
                "u32" => i64::from(u32::MAX),
                _ => i64::MAX,
            };
            if n > max {
                return Err(crate::core::error::ConfigError::InvalidValue {
                    key: String::new(),
                    message: format!("{n} is above {max}"),
                });
            }
            Ok(toml::Value::Integer(n))
        }
        "f32" | "f64" => {
            let n: f64 =
                value
                    .parse()
                    .map_err(|_| crate::core::error::ConfigError::ExpectedNumber {
                        value: value.to_string(),
                    })?;
            Ok(toml::Value::Float(n))
        }
        "string" | "string?" => Ok(toml::Value::String(value.to_string())),
        "string[]" => {
            let items: Vec<toml::Value> = value
                .split(',')
                .map(|s| toml::Value::String(s.trim().to_string()))
                .filter(|v| v.as_str() != Some(""))
                .collect();
            Ok(toml::Value::Array(items))
        }
        other => Err(crate::core::error::ConfigError::Message(format!(
            "no parser for the schema type '{other}'"
        ))),
    }
}

/// Keys whose value has bounds the type does not express. A value outside
/// them is refused instead of being clamped where it is read.
fn check_range(key: &str, value: &toml::Value) -> Result<(), crate::core::error::ConfigError> {
    if key == "proxy.port" && value.as_integer() == Some(0) {
        return Err(crate::core::error::ConfigError::InvalidValue {
            key: key.to_string(),
            message: "the port must be between 1 and 65535".to_string(),
        });
    }
    let bounds = match key {
        "compression_level" => Some((0.0, 1.0)),
        _ => None,
    };
    if let (Some((lo, hi)), Some(n)) = (bounds, value.as_float())
        && !(lo..=hi).contains(&n)
    {
        return Err(crate::core::error::ConfigError::InvalidValue {
            key: key.to_string(),
            message: format!("{n} is outside {lo}–{hi}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_values() {
        let schema = KeySchema {
            ty: "bool".to_string(),
            default: serde_json::json!(false),
            description: String::new(),
        };
        assert_eq!(
            parse_value("true", &schema).unwrap(),
            toml::Value::Boolean(true)
        );
        assert_eq!(
            parse_value("false", &schema).unwrap(),
            toml::Value::Boolean(false)
        );
        assert!(parse_value("maybe", &schema).is_err());
    }

    /// `proxy.port` is a `u16?` and parses as a number, bounded by the
    /// type; `compression_level` outside 0–1 is refused, not clamped.
    #[test]
    fn optional_port_parses_and_ranges_are_enforced() {
        let port = KeySchema {
            ty: "u16?".to_string(),
            default: serde_json::json!(null),
            description: String::new(),
        };
        assert_eq!(
            parse_value("18999", &port).unwrap(),
            toml::Value::Integer(18999)
        );
        assert!(parse_value("70000", &port).is_err());
        assert!(check_range("compression_level", &toml::Value::Float(7.0)).is_err());
        assert!(check_range("compression_level", &toml::Value::Float(0.5)).is_ok());
    }

    #[test]
    fn parse_integer_values() {
        let schema = KeySchema {
            ty: "u32".to_string(),
            default: serde_json::json!(0),
            description: String::new(),
        };
        assert_eq!(
            parse_value("42", &schema).unwrap(),
            toml::Value::Integer(42)
        );
        assert!(parse_value("-1", &schema).is_err());
        assert!(parse_value("abc", &schema).is_err());
    }

    /// Every type the schema uses has a parser.
    #[test]
    fn every_schema_type_parses() {
        for section in ConfigSchema::generate().sections.values() {
            for key in section.keys.values() {
                let sample = match key.ty.trim_end_matches('?') {
                    "bool" => "true",
                    "string" | "string[]" => "x",
                    _ => "1",
                };
                assert!(parse_value(sample, key).is_ok(), "{}", key.ty);
            }
        }
    }

    #[test]
    fn parse_string_array() {
        let schema = KeySchema {
            ty: "string[]".to_string(),
            default: serde_json::json!([]),
            description: String::new(),
        };
        let result = parse_value("a, b, c", &schema).unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_str().unwrap(), "a");
        assert_eq!(arr[2].as_str().unwrap(), "c");
    }

    #[test]
    fn display_toml_value_renders_user_facing_form() {
        // Strings drop their quotes (what a user would type on the CLI).
        assert_eq!(
            display_toml_value(&toml::Value::String("enforce".into())),
            "enforce"
        );
        assert_eq!(display_toml_value(&toml::Value::Boolean(false)), "false");
        assert_eq!(display_toml_value(&toml::Value::Integer(8080)), "8080");
        assert_eq!(
            display_toml_value(&toml::Value::Array(vec![
                toml::Value::String("a".into()),
                toml::Value::String("b".into()),
            ])),
            "a, b"
        );
    }

    fn config_text() -> String {
        std::fs::read_to_string(Config::path().unwrap()).unwrap_or_default()
    }

    /// Setting a key to its default, or to `default`, removes it; the
    /// file's comments stay.
    #[test]
    fn the_default_value_removes_the_key_and_comments_stay() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let path = Config::path().unwrap();
        std::fs::write(&path, "# mine\ncompression_level = 0.7 # dense\n").unwrap();

        set_by_key("compression_level", "0.5").unwrap();
        assert_eq!(config_text(), "# mine\ncompression_level = 0.5 # dense\n");
        set_by_key("compression_level", "0.3").unwrap();
        assert!(
            !config_text().contains("compression_level"),
            "{}",
            config_text()
        );
        assert!(config_text().contains("# mine"));

        set_by_key("proxy.upstreams.anthropic", "http://127.0.0.1:11434/").unwrap();
        assert!(config_text().contains("anthropic = \"http://127.0.0.1:11434\""));
        set_by_key("proxy.upstreams.anthropic", "default").unwrap();
        let text = config_text();
        assert!(
            !text.contains("anthropic") && !text.contains("upstreams"),
            "{text}"
        );
    }

    /// A value the loader would drop is refused here.
    #[test]
    fn values_the_loader_would_drop_are_refused() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        for url in ["127.0.0.1:11434", "notaurl", "ftp://x.y"] {
            assert!(
                set_by_key("proxy.upstreams.anthropic", url).is_err(),
                "{url}"
            );
        }
        assert!(set_by_key("proxy.port", "0").is_err());
        assert!(set_by_key("proxy.upstreams.gemini", "https://x.example").is_err());
        assert!(set_by_key("proxy.upstreams", "x").is_err());
        assert_eq!(config_text(), "");
    }
}
