//! Install/environment wiring: MCP registration, editor
//! surfaces, session state, CWD sanity, LSP servers.

#[allow(clippy::wildcard_imports)]
use crate::doctor::common::*;
use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST, YELLOW};

pub(crate) fn mcp_config_outcome() -> Outcome {
    let Some(home) = dirs::home_dir() else {
        return Outcome {
            ok: false,
            line: format!("{BOLD}MCP config{RST}  {RED}could not resolve home directory{RST}"),
        };
    };
    mcp_config_check(&home)
}

/// Which agent configs hold sobre's MCP entry. None is fine: the hooks work
/// without the MCP server, which `init` connects only on request.
pub(crate) fn mcp_config_check(home: &std::path::Path) -> Outcome {
    let location_names = sobre_mcp_location_names(home);
    let mut found: Vec<String> = mcp_config_locations(home)
        .into_iter()
        .filter(|loc| location_names.contains(loc.name))
        .map(|loc| format!("{} {DIM}({}){RST}", loc.name, loc.display))
        .collect();
    found.sort();
    found.dedup();

    if found.is_empty() {
        Outcome {
            ok: true,
            line: format!(
                "{BOLD}MCP server{RST}  {DIM}not connected (opt-in: sobre init --mcp){RST}"
            ),
        }
    } else {
        Outcome {
            ok: true,
            line: format!(
                "{BOLD}MCP config{RST}  {GREEN}sobre found in: {}{RST}",
                found.join(", ")
            ),
        }
    }
}
/// WSL2 + VS Code: VS Code's start-on-demand MCP lifecycle has a
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
    if !sobre_mcp_location_names(&home).contains("VS Code") {
        return None;
    }
    Some(Outcome {
        ok: true,
        line: format!(
            "{BOLD}WSL2 + VS Code{RST}  {YELLOW}known first-call race in VS Code's MCP client{RST}  {DIM}(a fresh conversation's first sobre_* call may fail with \"reading 'invoke'\" — retry succeeds; upstream: microsoft/vscode#321150. Mitigation: run the MCP server once via \"MCP: List Servers\" → sobre → Start){RST}"
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
        // live (doctor run from a shell whose cwd is not one of the
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
/// across ALL workspaces (multi-window setups). `None` when no
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
    use crate::lsp::config::KNOWN_SERVERS;

    KNOWN_SERVERS
        .iter()
        .map(|info| {
            let found = which::which(info.binary);
            match found {
                Ok(path) => Outcome {
                    ok: true,
                    line: format!(
                        "{BOLD}{}{RST}  {GREEN}✓ {}{RST}  {DIM}{}{RST}",
                        info.language,
                        info.binary,
                        path.display()
                    ),
                },
                Err(_) => Outcome {
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
