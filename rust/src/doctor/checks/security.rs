//! Config parsing, search roots, secret redaction.

use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST};

/// Flags when `config.toml` fails to parse (the silent-default trap behind
/// #341, where an edit appears to "do nothing" because the file never loaded),
/// and keys that no setting reads: they are ignored, so a typo would
/// otherwise go unnoticed.
pub(crate) fn config_parse_outcome() -> Outcome {
    if let Some(err) = crate::core::config::last_config_parse_error() {
        let short = err.lines().next().unwrap_or("parse error");
        return Outcome {
            ok: false,
            line: format!(
                "{BOLD}Config file{RST}  {RED}config.toml fails to parse → running on DEFAULTS{RST}  {DIM}({short}){RST}"
            ),
        };
    }
    let raw = crate::core::config::Config::path().and_then(|p| std::fs::read_to_string(p).ok());
    let unknown = raw.as_deref().map(unknown_config_keys).unwrap_or_default();
    if !unknown.is_empty() {
        return Outcome {
            ok: false,
            line: format!(
                "{BOLD}Config file{RST}  {RED}{} unknown key(s), ignored{RST}  {DIM}({}){RST}",
                unknown.len(),
                unknown.join(", ")
            ),
        };
    }
    Outcome {
        ok: true,
        line: format!("{BOLD}Config file{RST}  {GREEN}parses{RST}"),
    }
}

/// Keys of a `config.toml` document that no setting reads, each with the
/// closest known key when one is near, so `[proxy] prot = 4444` is flagged.
pub(crate) fn unknown_config_keys(raw: &str) -> Vec<String> {
    fn flatten(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
        for (k, v) in table {
            let full = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                toml::Value::Table(sub) => flatten(sub, &full, out),
                _ => out.push(full),
            }
        }
    }

    let Ok(table) = raw.parse::<toml::Table>() else {
        return Vec::new();
    };
    let known = crate::core::config::schema::ConfigSchema::generate().known_keys();
    let mut keys = Vec::new();
    flatten(&table, "", &mut keys);
    keys.into_iter()
        .filter(|k| !known.contains(k))
        .map(
            |k| match crate::core::levenshtein::closest(&k, known.iter().map(String::as_str)) {
                Some(near) => format!("{k}, did you mean {near}?"),
                None => k,
            },
        )
        .collect()
}

/// Reports configured `search_roots` that never resolve (GH #392): an
/// unexpanded `$VAR`, a typo or a missing directory is otherwise silent.
pub(crate) fn search_roots_outcome() -> Outcome {
    let cfg = crate::core::config::Config::load();
    let dead: Vec<&String> = cfg
        .search_roots
        .iter()
        .filter(|raw| !crate::core::pathutil::expand_user_path(raw).is_dir())
        .collect();
    if !dead.is_empty() {
        return Outcome {
            ok: false,
            line: format!(
                "{BOLD}Search roots{RST}  {RED}{} entr{} never searched{RST}  {DIM}({} — unset $VAR or missing directory){RST}",
                dead.len(),
                if dead.len() == 1 { "y is" } else { "ies are" },
                dead.iter()
                    .map(|d| d.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }
    let detail = match cfg.search_roots.len() {
        0 => "project root only".to_string(),
        n => format!("project root + {n} search root(s)"),
    };
    Outcome {
        ok: true,
        line: format!("{BOLD}Search roots{RST}  {GREEN}ok{RST}  {DIM}({detail}){RST}"),
    }
}

/// Managed ONNX Runtime state (GH #732). Only rendered on embedding-enabled
/// builds; `None` when the runtime is simply not provisioned (embeddings are
/// opt-in — a missing optional runtime is not a finding, the provision hint
/// lives in the embeddings CLI and the resolver error).
pub(crate) fn managed_ort_outcome() -> Option<Outcome> {
    if !cfg!(feature = "dense") {
        return None;
    }
    let path = crate::core::artifacts_exec::ort_provision::managed_dylib_path()?;
    let ok = std::fs::File::open(&path).is_ok();
    Some(Outcome {
        ok,
        line: if ok {
            format!(
                "{BOLD}Managed ONNX Runtime{RST}  {GREEN}{}{RST}  {DIM}({}){RST}",
                crate::core::artifacts_exec::ort_provision::ORT_VERSION,
                path.display()
            )
        } else {
            format!(
                "{BOLD}Managed ONNX Runtime{RST}  {RED}unreadable{RST}  {DIM}({} — fix: sobre embeddings provision --force){RST}",
                path.display()
            )
        },
    })
}
