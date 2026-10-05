//! Containment & security: shell gate, path jail, secret redaction.

use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST, YELLOW};

/// Reports the shell gate — and, crucially, flags when `config.toml` fails to
/// parse (the silent-default trap behind #341, where an edit appears to "do
/// nothing" because the file never loaded).
pub(crate) fn shell_gate_outcome() -> Outcome {
    if let Some(err) = crate::core::config::last_config_parse_error() {
        let short = err.lines().next().unwrap_or("parse error");
        return Outcome {
            ok: false,
            line: format!(
                "{BOLD}Shell gating{RST}  {RED}config.toml fails to parse → running on DEFAULTS{RST}  {DIM}({short}){RST}"
            ),
        };
    }

    Outcome {
        ok: true,
        line: format!(
            "{BOLD}Shell gating{RST}  {GREEN}enforce{RST}  {DIM}(every command allowed; eval/exec/source and interpreter-eval blocked){RST}"
        ),
    }
}
/// Reports the effective PathJail state (GH #392): whether it is open, and
/// whether configured `allow_paths` entries
/// actually resolve — the silent failure mode behind "allow_paths has no
/// effect" reports (unexpanded `$VAR`, typos, paths that don't exist).
pub(crate) fn path_jail_outcome() -> Outcome {
    if cfg!(feature = "no-jail") {
        return Outcome {
            ok: true,
            line: format!(
                "{BOLD}Path jail{RST}  {YELLOW}disabled at compile time{RST}  {DIM}(built with the no-jail feature){RST}"
            ),
        };
    }

    let cfg = crate::core::config::Config::load();
    let entries: Vec<&String> = cfg.allow_paths.iter().collect();
    let mut grants_everything = false;
    let mut dead: Vec<String> = Vec::new();
    for raw in &entries {
        let expanded = crate::core::pathjail::expand_user_path(raw);
        if expanded == std::path::Path::new("/") {
            grants_everything = true;
        }
        if !expanded.exists() {
            dead.push((*raw).clone());
        }
    }

    if grants_everything {
        return Outcome {
            ok: true,
            line: format!(
                "{BOLD}Path jail{RST}  {YELLOW}open{RST}  {DIM}(allow_paths contains \"/\" — all tool paths allowed){RST}"
            ),
        };
    }
    if !dead.is_empty() {
        return Outcome {
            ok: false,
            line: format!(
                "{BOLD}Path jail{RST}  {RED}{} allow_paths entr{} never match{RST}  {DIM}({} — unset $VAR or missing path){RST}",
                dead.len(),
                if dead.len() == 1 {
                    "y will"
                } else {
                    "ies will"
                },
                dead.join(", ")
            ),
        };
    }
    let detail = if entries.is_empty() {
        let cfg = crate::core::config::Config::path()
            .map_or_else(|| "config.toml".to_string(), |p| p.display().to_string());
        format!("project root only; extend via allow_paths in {cfg}")
    } else {
        format!("project root + {} configured allow path(s)", entries.len())
    };

    // Env-channel relaxations the config view above can't see (inherited from the
    // multi-root workspace). Surface them as a standing security note (GH security
    // audit, finding 3); no-jail / path_jail=false are handled by the early
    // returns above, so only those relaxations reach here.
    let relaxed: Vec<&str> = crate::core::pathjail::active_relaxations()
        .iter()
        .map(|r| r.source)
        .collect();
    if relaxed.is_empty() {
        Outcome {
            ok: true,
            line: format!("{BOLD}Path jail{RST}  {GREEN}active{RST}  {DIM}({detail}){RST}"),
        }
    } else {
        Outcome {
            ok: true,
            line: format!(
                "{BOLD}Path jail{RST}  {GREEN}active{RST} {YELLOW}but relaxed via {}{RST}  {DIM}({detail}; relaxations widen access beyond the project root){RST}",
                relaxed.join(", ")
            ),
        }
    }
}
/// Reports secret/`.env` redaction — the exfiltration-defense plane that is
/// deliberately independent of the path jail + shell gating (#507), so it gets
/// its own line: "are my API keys masked before they reach the LLM provider?".
pub(crate) fn secret_detection_outcome() -> Outcome {
    let sd = &crate::core::config::SecretDetectionConfig::default();
    if !sd.enabled {
        return Outcome {
            ok: true,
            line: format!(
                "{BOLD}Secret redaction{RST}  {YELLOW}off{RST}  {DIM}(secret_detection.enabled=false — .env/API keys can reach the provider; re-enable: lean-ctx security secrets on){RST}"
            ),
        };
    }
    if !sd.redact {
        return Outcome {
            ok: true,
            line: format!(
                "{BOLD}Secret redaction{RST}  {YELLOW}detect-only{RST}  {DIM}(secrets flagged but not masked — set secret_detection.redact=true to mask){RST}"
            ),
        };
    }
    let custom = if sd.custom_patterns.is_empty() {
        String::new()
    } else {
        format!(" + {} custom pattern(s)", sd.custom_patterns.len())
    };
    Outcome {
        ok: true,
        line: format!(
            "{BOLD}Secret redaction{RST}  {GREEN}on{RST}  {DIM}(.env/API keys masked before the model sees them{custom}){RST}"
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
