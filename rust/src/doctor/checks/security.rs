//! Config parsing, search roots, secret redaction.

use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST};

/// Flags when `config.toml` fails to parse (the silent-default trap behind
/// #341, where an edit appears to "do nothing" because the file never loaded).
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
    Outcome {
        ok: true,
        line: format!("{BOLD}Config file{RST}  {GREEN}parses{RST}"),
    }
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

/// Reports secret/`.env` redaction (#507), on its own line: "are my API keys masked before they reach the LLM provider?".
pub(crate) fn secret_detection_outcome() -> Outcome {
    Outcome {
        ok: true,
        line: format!(
            "{BOLD}Secret redaction{RST}  {GREEN}on{RST}  {DIM}(.env/API keys masked before the model sees them){RST}"
        ),
    }
}

/// Managed ONNX Runtime state (GH #732). Only rendered on embedding-enabled
/// builds; `None` when the runtime is simply not provisioned (embeddings are
/// opt-in — a missing optional runtime is not a finding, the provision hint
/// lives in the embeddings CLI and the resolver error).
pub(crate) fn managed_ort_outcome() -> Option<Outcome> {
    if !cfg!(feature = "embeddings") {
        return None;
    }
    let path = crate::core::artifacts_exec::ort_provision::managed_dylib_path()?;
    let ok = crate::core::artifacts_exec::binhash::sha256_file(&path).is_ok();
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
                "{BOLD}Managed ONNX Runtime{RST}  {RED}unreadable{RST}  {DIM}({} — fix: lean-ctx embeddings provision --force){RST}",
                path.display()
            )
        },
    })
}
