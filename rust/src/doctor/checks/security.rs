//! Containment & security posture: shell gate, path jail, workspace
//! trust, secret redaction, IDE permission inheritance.

use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST, YELLOW};

/// Reports the shell gate exactly as the MCP tools enforce it — and, crucially,
/// flags when `config.toml` fails to parse (the silent-default trap behind #341,
/// where a `shell_security` edit appears to "do nothing" because the file never loaded).
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

    // GL #788: surface a relaxed posture loudly — an unexpected `off`/`warn`
    // silently drops the unconditional blocks, which is the only gating left.
    match crate::core::shell_gate::ShellSecurity::resolve() {
        crate::core::shell_gate::ShellSecurity::Off => {
            return Outcome {
                ok: true,
                line: format!(
                    "{BOLD}Shell gating{RST}  {YELLOW}off{RST}  {DIM}(shell_security=off — even the unconditional blocks are skipped){RST}"
                ),
            };
        }
        crate::core::shell_gate::ShellSecurity::Warn => {
            return Outcome {
                ok: true,
                line: format!(
                    "{BOLD}Shell gating{RST}  {YELLOW}warn-only{RST}  {DIM}(shell_security=warn — violations logged, never blocked){RST}"
                ),
            };
        }
        crate::core::shell_gate::ShellSecurity::Enforce => {}
    }

    Outcome {
        ok: true,
        line: format!(
            "{BOLD}Shell gating{RST}  {GREEN}enforce{RST}  {DIM}(every command allowed; eval/exec/source and interpreter-eval blocked){RST}"
        ),
    }
}
/// Reports the effective PathJail state (GH #392): which knob (if any)
/// disabled it, and whether configured `allow_paths`/`extra_roots` entries
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
    if cfg.path_jail == Some(false) {
        return Outcome {
            ok: true,
            line: format!(
                "{BOLD}Path jail{RST}  {YELLOW}disabled{RST}  {DIM}(path_jail = false in config.toml — all tool paths allowed){RST}"
            ),
        };
    }

    let entries: Vec<&String> = cfg
        .allow_paths
        .iter()
        .chain(cfg.extra_roots.iter())
        .collect();
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
                "{BOLD}Path jail{RST}  {YELLOW}active, but allow_paths contains \"/\"{RST}  {DIM}(grants everything — prefer the explicit `path_jail = false`){RST}"
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
    // multi-root workspace or `allow_ide_config_dirs`). Surface them as a standing security note (GH security
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
/// deliberately independent of the path jail + shell gating (#507). A user
/// can run `lean-ctx yolo` (containment off) and still have this on, so it gets
/// its own line: "are my API keys masked before they reach the LLM provider?".
pub(crate) fn secret_detection_outcome() -> Outcome {
    let cfg = crate::core::config::Config::load();
    let sd = &cfg.secret_detection;
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
/// Reports IDE permission inheritance: when on, lean-ctx mirrors the host IDE's
/// bash/read/edit/grep permission rules onto its own tools, so `ctx_shell` honors
/// a `rm *: ask`/`deny` rule instead of forming a parallel, ungoverned path.
pub(crate) fn permission_inheritance_outcome() -> Outcome {
    use crate::core::config::{Config, PermissionInheritance};
    let cfg = Config::load();
    if cfg.permission_inheritance_effective() != PermissionInheritance::On {
        return Outcome {
            ok: true,
            line: format!(
                "{BOLD}Permission inheritance{RST}  {YELLOW}off{RST}  {DIM}(enable: lean-ctx config set permission_inheritance on → ctx_shell honors your IDE's bash/rm rules){RST}"
            ),
        };
    }
    let policy = dirs::home_dir()
        .map(|home| crate::core::ide_permissions::load_opencode(&home, None))
        .unwrap_or_default();
    let detail = if policy.is_empty() {
        "on, but no OpenCode permission rules found yet".to_string()
    } else {
        format!(
            "mirroring {} OpenCode permission rule(s)",
            policy.rule_count()
        )
    };
    Outcome {
        ok: true,
        line: format!("{BOLD}Permission inheritance{RST}  {GREEN}on{RST}  {DIM}({detail}){RST}"),
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
