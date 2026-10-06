//! Auto-generated config schema from `Config` struct metadata.
//!
//! Used by `lean-ctx config schema` to emit JSON and by
//! `lean-ctx config validate` to check user config.toml files.

use serde::Serialize;
use std::collections::BTreeMap;
mod sections_advanced;
mod sections_core;

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSchema {
    pub version: u32,
    pub sections: BTreeMap<String, SectionSchema>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionSchema {
    pub description: String,
    pub keys: BTreeMap<String, KeySchema>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeySchema {
    #[serde(rename = "type")]
    pub ty: String,
    pub default: serde_json::Value,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

fn key(ty: &str, default: serde_json::Value, desc: &str) -> KeySchema {
    KeySchema {
        ty: ty.to_string(),
        default,
        description: desc.to_string(),
        values: None,
    }
}

impl ConfigSchema {
    pub fn generate() -> Self {
        let mut sections = BTreeMap::new();
        sections_core::build(&mut sections);
        sections_advanced::build(&mut sections);

        ConfigSchema {
            version: 1,
            sections,
        }
    }

    /// Looks up a key schema by its dot-separated TOML path.
    /// Returns `None` if the key is not part of the schema.
    pub fn lookup(&self, key: &str) -> Option<&KeySchema> {
        if let Some(dot_pos) = key.find('.') {
            let section = &key[..dot_pos];
            let field = &key[dot_pos + 1..];
            self.sections.get(section)?.keys.get(field)
        } else {
            self.sections.get("root")?.keys.get(key)
        }
    }

    /// All known TOML keys (dot-separated) for validation.
    ///
    /// Combines the hand-written schema (which carries descriptions, types and
    /// help text) with the keys derived from the live `Config` struct. The
    /// struct is the source of truth for *what is valid*, so a field added to
    /// `Config` is recognised by `config apply` / `config validate` immediately,
    /// without anyone remembering to mirror it into `sections_*.rs` (#456).
    pub fn known_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for (section, schema) in &self.sections {
            if section == "root" {
                for key_name in schema.keys.keys() {
                    keys.push(key_name.clone());
                }
            } else {
                if schema.keys.is_empty() {
                    keys.push(section.clone());
                }
                for key_name in schema.keys.keys() {
                    keys.push(format!("{section}.{key_name}"));
                }
            }
        }
        keys.extend(config_derived_keys());
        keys.sort();
        keys.dedup();
        keys
    }
}

/// Every TOML key the `Config` struct serialises to, in dot-separated form
/// (e.g. `allow_paths`, `proxy`, `proxy.port`). Derived from
/// `Config::default()` so validation tracks the struct automatically (#456).
///
/// Option fields that default to `None` are omitted by serde and therefore not
/// listed here; those keys still come from the hand-written schema. Emitting the
/// bare section name (e.g. `memory`) lets the `starts_with("section.")` rule in
/// the validators accept the whole section, matching how empty schema sections
/// already behave.
fn config_derived_keys() -> Vec<String> {
    fn walk(table: &toml::value::Table, prefix: &str, out: &mut Vec<String>) {
        for (k, v) in table {
            let full = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            if let toml::Value::Table(sub) = v {
                out.push(full.clone());
                walk(sub, &full, out);
            } else {
                out.push(full);
            }
        }
    }

    let mut out = Vec::new();
    if let Ok(toml::Value::Table(table)) =
        toml::Value::try_from(crate::core::config::Config::default())
    {
        walk(&table, "", &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fields serde omits from `Config::default()` because they carry
    /// `skip_serializing_if` and are empty by default. They are real fields —
    /// they simply cannot be observed through a serialised default — so the
    /// reverse-parity test below has to know about them. Adding a
    /// `skip_serializing_if` field to a config struct means adding it here.
    const SERDE_OMITTED_BY_DEFAULT: &[&str] = &["compression_level"];

    /// `Config::default()` as JSON rather than TOML: JSON keeps `Option::None`
    /// as `null`, so the `Option` fields that TOML drops stay visible.
    fn config_as_json() -> serde_json::Value {
        serde_json::to_value(crate::core::config::Config::default())
            .expect("Config must serialise to JSON")
    }

    /// Walk a dotted path into a JSON value. `None` when any segment is absent.
    fn resolve<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
        let mut cur = root;
        for seg in path.split('.') {
            cur = cur.as_object()?.get(seg)?;
        }
        Some(cur)
    }

    /// The reverse of [`known_keys_cover_every_config_struct_field`], and the
    /// direction that actually bit.
    ///
    /// A `sections_*.rs` entry that hardcodes its default — `json!(false)`,
    /// `json!([])` — instead of reading `cfg.<field>` keeps compiling after the
    /// struct field is deleted. The key then stays in `config schema`, in
    /// `config validate` and in the published
    /// `docs/reference/generated/config-keys.md` while nothing reads it: a
    /// setting the user can write that silently does nothing. `shell_allowlist`,
    /// `shell_allowlist_extra` and `update_check_disabled` each survived their
    /// field exactly that way.
    ///
    /// The compiler cannot see it — a hardcoded `json!` value has no link to the
    /// struct — so it takes a test.
    #[test]
    fn every_schema_key_has_a_config_field() {
        let schema = ConfigSchema::generate();
        let cfg = config_as_json();

        let mut orphans: Vec<String> = Vec::new();
        for (section, sec) in &schema.sections {
            let section_value = if section == "root" {
                Some(&cfg)
            } else {
                resolve(&cfg, section)
            };
            match section_value {
                None => orphans.push(format!("[{section}] (whole section)")),
                // A section that is a list: its schema keys describe the shape
                // of one *entry*, not fields on `Config`. Same for any section
                // that is not a plain object.
                Some(v) if !v.is_object() => {}
                Some(_) => {
                    for name in sec.keys.keys() {
                        let path = if section == "root" {
                            name.clone()
                        } else {
                            format!("{section}.{name}")
                        };
                        // Resolve the whole path so a dotted key name works
                        // too, rather than looking `name` up in the section.
                        if resolve(&cfg, &path).is_none()
                            && !SERDE_OMITTED_BY_DEFAULT.contains(&path.as_str())
                            && !path.starts_with("proxy.upstreams.")
                        {
                            orphans.push(path);
                        }
                    }
                }
            }
        }

        assert!(
            orphans.is_empty(),
            "schema declares {} key(s) with no matching field on `Config`: {orphans:#?}
             Either the struct field was deleted and the `sections_*.rs` entry              outlived it (delete the entry), or the field carries              `skip_serializing_if` and is empty by default (add it to              SERDE_OMITTED_BY_DEFAULT, above, with the reason).",
            orphans.len(),
        );
    }

    /// Mirrors the acceptance rule used by `config validate` / `config apply`:
    /// a key is valid if it is listed verbatim or sits under a known section.
    fn accepted(known: &[String], key: &str) -> bool {
        known.iter().any(|k| k == key) || known.iter().any(|k| key.starts_with(&format!("{k}.")))
    }

    /// #456: every field the `Config` struct actually serialises must be
    /// accepted by validation, or the hand-written schema has drifted.
    #[test]
    fn known_keys_cover_every_config_struct_field() {
        let known = ConfigSchema::generate().known_keys();
        let missing: Vec<_> = config_derived_keys()
            .into_iter()
            .filter(|k| !accepted(&known, k))
            .collect();
        assert!(
            missing.is_empty(),
            "config struct fields not recognised by validation (schema drift): {missing:?}"
        );
    }

    /// Spot-check the concrete keys from the #456 report so a future schema/struct
    /// refactor that reintroduces the drift fails loudly.
    #[test]
    fn known_keys_recognise_reported_456_keys() {
        let known = ConfigSchema::generate().known_keys();
        for key in ["allow_paths", "proxy"] {
            assert!(
                accepted(&known, key),
                "validation must recognise '{key}' (#456)"
            );
        }
    }

    /// `config set` resolves keys via [`ConfigSchema::lookup`] — the hand-written
    /// schema only, NOT `known_keys()` (which also folds in `config_derived_keys`).
    /// An `Option<_>` scalar field defaults to `None`, so serde omits it from
    /// `Config::default()` and it never appears in `config_derived_keys`: such a
    /// field is settable via `config set` **only** if it was hand-added to a
    /// `sections_*.rs` schema. Forgetting that is the `Unknown config key: <x>`
    /// regression a user hit before #507. Guard the whole
    /// class so a new `Option` knob can't silently become un-settable again — if you add an `Option` scalar to
    /// `Config`, register it in `sections_*.rs` and list it here.
    #[test]
    fn option_scalar_keys_are_cli_settable() {
        let schema = ConfigSchema::generate();
        for key in [
            "shell_timeout_secs",
            "compression_level",
            "proxy.enabled",
            "proxy.port",
        ] {
            assert!(
                schema.lookup(key).is_some(),
                "`lean-ctx config set {key} <v>` fails with 'Unknown config key' — \
                 add `{key}` to a sections_*.rs schema"
            );
        }
    }
}

/// INVARIANTS.md §2: at most 15 configuration keys. A key is a top-level field
/// or a field of `[proxy]`; a table a key holds (`proxy.upstreams`,
/// `proxy.routing`) counts once, whatever it contains. Adding a key means
/// updating this list, which is the point: it must be a deliberate change.
#[cfg(test)]
mod key_cap {
    const KEY_CAP: usize = 15;
    const KEYS: &[&str] = &[
        "allow_paths",
        "compression_level",
        "debug_log",
        "embedding_model",
        "hook_binary",
        "ignore",
        "passthrough_commands",
        "proxy.enabled",
        "proxy.port",
        "proxy.routing",
        "proxy.upstreams",
        "read_only_roots",
        "shell_timeout_secs",
    ];

    #[test]
    fn config_keys_are_the_documented_set_and_under_the_cap() {
        let mut counted: Vec<String> = super::ConfigSchema::generate()
            .known_keys()
            .into_iter()
            .filter(|k| k != "proxy")
            .map(|k| {
                let parts: Vec<&str> = k.split('.').collect();
                if parts[0] == "proxy" && parts.len() > 2 {
                    format!("proxy.{}", parts[1])
                } else {
                    k
                }
            })
            .collect();
        counted.sort();
        counted.dedup();
        assert_eq!(counted, KEYS, "the configuration keys changed");
        assert!(KEYS.len() <= KEY_CAP, "{} keys, cap {KEY_CAP}", KEYS.len());
    }
}
