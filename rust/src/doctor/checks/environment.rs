//! Install/environment wiring: MCP registration, editor
//! surfaces, session state, CWD sanity, LSP servers.

#[allow(clippy::wildcard_imports)]
use crate::doctor::common::*;
use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST, YELLOW};

/// Cognition v2 activation: how many science-backed subsystems have actually
/// fired on this install. Proves the stack is wired (not dead code) without
/// needing external instrumentation — `lean-ctx introspect cognition` drills in.
pub(crate) fn cognition_activity_outcome() -> Outcome {
    let snap = crate::core::introspect::snapshot();
    let total = snap.len();
    let active = snap.iter().filter(|(_, a)| a.count > 0).count();
    // Before any tool calls have run nothing has fired yet — neutral, not a
    // failure. Always pass; the value is the visibility, not a gate.
    let line = if active == 0 {
        format!(
            "{BOLD}Cognition{RST}  {DIM}no activity recorded yet{RST}  {DIM}(inspect: lean-ctx introspect cognition){RST}"
        )
    } else {
        format!(
            "{BOLD}Cognition{RST}  {GREEN}{active}/{total} subsystems active{RST}  {DIM}(details: lean-ctx introspect cognition){RST}"
        )
    };
    Outcome { ok: true, line }
}
/// Reports the format-aware passthrough (#342): output already in a compact,
/// token-oriented format (TOON by default) is preserved verbatim instead of
/// recompressed, so an agent's proof-of-output-shape survives intact.
pub(crate) fn compact_format_passthrough_outcome() -> Outcome {
    Outcome {
        ok: true,
        line: format!(
            "{BOLD}Compact-format passthrough{RST}  {GREEN}{}{RST}  {DIM}(preserved verbatim, not recompressed){RST}",
            crate::core::config::PRESERVE_COMPACT_FORMATS.join(", ")
        ),
    }
}
pub(crate) fn mcp_config_outcome() -> Outcome {
    let Some(home) = dirs::home_dir() else {
        return Outcome {
            ok: false,
            line: format!("{BOLD}MCP config{RST}  {RED}could not resolve home directory{RST}"),
        };
    };

    let locations = mcp_config_locations(&home);
    let location_names = lean_ctx_mcp_location_names(&home);
    let mut found: Vec<String> = Vec::new();
    let mut exists_no_ref: Vec<String> = Vec::new();

    for loc in &locations {
        if std::fs::read_to_string(&loc.path).is_ok() {
            if location_names.contains(loc.name) {
                found.push(format!("{} {DIM}({}){RST}", loc.name, loc.display));
            } else {
                exists_no_ref.push(loc.name.to_string());
            }
        }
    }

    found.sort();
    found.dedup();
    exists_no_ref.sort();
    exists_no_ref.dedup();

    if !found.is_empty() {
        Outcome {
            ok: true,
            line: format!(
                "{BOLD}MCP config{RST}  {GREEN}lean-ctx found in: {}{RST}",
                found.join(", ")
            ),
        }
    } else if !exists_no_ref.is_empty() {
        let has_claude = exists_no_ref.iter().any(|n| n.starts_with("Claude Code"));
        let cause = if has_claude {
            format!(
                "{DIM}(Claude Code may overwrite ~/.claude.json on startup — lean-ctx entry missing from mcpServers){RST}"
            )
        } else {
            String::new()
        };
        let hint = if has_claude {
            format!("{DIM}(run: lean-ctx doctor --fix OR lean-ctx init --agent claude){RST}")
        } else {
            format!("{DIM}(run: lean-ctx doctor --fix OR lean-ctx init){RST}")
        };
        Outcome {
            ok: false,
            line: format!(
                "{BOLD}MCP config{RST}  {YELLOW}config exists for {} but mcpServers does not contain lean-ctx{RST}  {cause} {hint}",
                exists_no_ref.join(", "),
            ),
        }
    } else {
        Outcome {
            ok: false,
            line: format!(
                "{BOLD}MCP config{RST}  {YELLOW}no MCP config found{RST}  {DIM}(run: lean-ctx init){RST}"
            ),
        }
    }
}
/// WSL2 + VS Code (GH #669): VS Code's start-on-demand MCP lifecycle has a
/// known client-side race — the first tool call of a fresh conversation can
/// fire against cached tool metadata before the server's implementation is
/// bound, failing with `Cannot read properties of undefined (reading 'invoke')`
/// (microsoft/vscode#321150). Cold starts on WSL2 widen that window. Purely
/// informational (ok: true): the defect is upstream, a retry always succeeds.
pub(crate) fn wsl_vscode_mcp_outcome() -> Option<Outcome> {
    if !crate::core::io_health::is_wsl() {
        return None;
    }
    let home = dirs::home_dir()?;
    if !lean_ctx_mcp_location_names(&home).contains("VS Code") {
        return None;
    }
    Some(Outcome {
        ok: true,
        line: format!(
            "{BOLD}WSL2 + VS Code{RST}  {YELLOW}known first-call race in VS Code's MCP client{RST}  {DIM}(a fresh conversation's first ctx_* call may fail with \"reading 'invoke'\" — retry succeeds; upstream: microsoft/vscode#321150. Mitigation: run the MCP server once via \"MCP: List Servers\" → lean-ctx → Start){RST}"
        ),
    })
}

pub(crate) fn session_state_outcome() -> Outcome {
    use crate::core::session::SessionState;

    match SessionState::load_latest() {
        Some(session) => {
            let root = session.project_root.as_deref().unwrap_or("(not set)");
            let cwd = session.shell_cwd.as_deref().unwrap_or("(not tracked)");
            Outcome {
                ok: true,
                line: format!(
                    "{BOLD}Session state{RST}  {GREEN}active{RST}  {DIM}root: {root}, cwd: {cwd}, v{}{RST}",
                    session.version
                ),
            }
        }
        // No session for THIS cwd — but sessions for other workspaces may be
        // live (GH #694: doctor run from a shell whose cwd is not one of the
        // open project roots claimed "no active session" and looked broken).
        // Surface the recent sessions with their roots instead of denying
        // their existence.
        None => match &recent_sessions_line(chrono::Utc::now()) {
            Some(line) => Outcome {
                ok: true,
                line: format!(
                    "{BOLD}Session state{RST}  {YELLOW}none for this directory{RST}  {DIM}{line}{RST}"
                ),
            },
            None => Outcome {
                ok: true,
                line: format!(
                    "{BOLD}Session state{RST}  {YELLOW}no active session{RST}  {DIM}(will be created on first tool call){RST}"
                ),
            },
        },
    }
}

/// Renders "recent: root1 (2h ago), root2 (5m ago)" for the newest sessions
/// across ALL workspaces (multi-window setups, GH #694). `None` when no
/// session exists at all.
fn recent_sessions_line(now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    format_recent_sessions(crate::core::session::SessionState::list_sessions(), now)
}

pub(crate) fn format_recent_sessions(
    sessions: Vec<crate::core::session::SessionSummary>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let mut seen_roots = std::collections::HashSet::new();
    let mut parts = Vec::new();
    for s in sessions {
        let Some(root) = s.project_root.filter(|r| !r.is_empty()) else {
            continue;
        };
        if !seen_roots.insert(root.clone()) {
            continue;
        }
        let display_root = std::path::Path::new(&root)
            .file_name()
            .map_or_else(|| root.clone(), |n| n.to_string_lossy().into_owned());
        parts.push(format!(
            "{display_root} ({})",
            humanize_age(now.signed_duration_since(s.updated_at))
        ));
        if parts.len() == 3 {
            break;
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("recent: {}", parts.join(", ")))
    }
}

pub(crate) fn humanize_age(age: chrono::Duration) -> String {
    let mins = age.num_minutes();
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 48 * 60 {
        format!("{}h ago", mins / 60)
    } else {
        format!("{}d ago", mins / (24 * 60))
    }
}
pub(crate) fn lsp_server_outcomes() -> Vec<Outcome> {
    use crate::lsp::config::{KNOWN_SERVERS, find_binary_in_path};

    KNOWN_SERVERS
        .iter()
        .map(|info| {
            let found = find_binary_in_path(info.binary);
            match found {
                Some(path) => Outcome {
                    ok: true,
                    line: format!(
                        "{BOLD}{}{RST}  {GREEN}✓ {}{RST}  {DIM}{}{RST}",
                        info.language,
                        info.binary,
                        path.display()
                    ),
                },
                None => Outcome {
                    ok: false,
                    line: format!(
                        "{BOLD}{}{RST}  {DIM}not installed{RST}  {YELLOW}{}{RST}",
                        info.language, info.install_hint
                    ),
                },
            }
        })
        .collect()
}
/// True when `cwd_str` points inside an IDE/agent config directory rather than a
/// real project (LM Studio, Claude, Codex). Matches both POSIX (`/`)
/// and Windows (`\`) separators so the warning fires on every platform.
pub(crate) fn cwd_looks_like_agent_dir(cwd_str: &str) -> bool {
    crate::core::pathutil::is_agent_config_dir(std::path::Path::new(cwd_str))
}

/// Warn if lean-ctx is running as an MCP server from a directory that lacks
/// a project marker and looks like an IDE/agent tool directory (e.g. .lmstudio,
/// .claude). This usually means the MCP client launched the process from the
/// wrong CWD, causing "path escapes project root" errors for every tool call.
pub(crate) fn mcp_server_cwd_outcome() -> Outcome {
    let is_mcp = crate::core::runtime_flags::mcp_server_enabled();
    if !is_mcp {
        return Outcome {
            ok: true,
            line: format!("{BOLD}MCP server CWD{RST}  {DIM}(not running as MCP server){RST}"),
        };
    }

    let Ok(cwd) = std::env::current_dir() else {
        return Outcome {
            ok: true,
            line: format!("{BOLD}MCP server CWD{RST}  {YELLOW}could not resolve{RST}"),
        };
    };

    let has_marker = crate::core::pathutil::has_project_marker(&cwd);
    let cwd_str = cwd.to_string_lossy();
    let suspicious = cwd_looks_like_agent_dir(&cwd_str);

    if has_marker {
        Outcome {
            ok: true,
            line: format!(
                "{BOLD}MCP server CWD{RST}  {GREEN}under project root{RST}  {DIM}{}{RST}",
                cwd.display()
            ),
        }
    } else if suspicious {
        Outcome {
            ok: false,
            line: format!(
                "{BOLD}MCP server CWD{RST}  {YELLOW}launched from an IDE/agent config dir{RST}  {DIM}lean-ctx was launched from {}, which is not a project root. It auto-corrects to your real project on the first absolute path (#580); for relative paths to resolve immediately, set `cwd` in your MCP client config to your project directory.{RST}",
                cwd.display()
            ),
        }
    } else {
        Outcome {
            ok: false,
            line: format!(
                "{BOLD}MCP server CWD{RST}  {YELLOW}no project marker found in CWD{RST}  {DIM}cwd={} — relative paths resolve against this directory. Add `cwd` to your MCP client config{RST}",
                cwd.display()
            ),
        }
    }
}
