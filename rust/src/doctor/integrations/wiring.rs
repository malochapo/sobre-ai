//! Shared low-level checks: MCP JSON entries, binary/hook reference
//! matching, rules files, activity probes.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn check_mcp_json(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return NamedCheck {
            name: "MCP config".to_string(),
            ok: false,
            detail: format!("missing ({})", path.display()),
        };
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();

    let Some(v) = parsed else {
        return NamedCheck {
            name: "MCP config".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };

    let entry = v.get("mcpServers").and_then(|m| m.get("sobre")).cloned();

    let Some(e) = entry else {
        return NamedCheck {
            name: "MCP config".to_string(),
            ok: false,
            detail: format!("sobre missing ({})", path.display()),
        };
    };

    let cmd_ok = e
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| cmd_matches_expected(c, binary));

    let ok = cmd_ok;
    let detail = if ok {
        format!("ok ({})", path.display())
    } else {
        format!("drift ({})", path.display())
    };
    NamedCheck {
        name: "MCP config".to_string(),
        ok,
        detail,
    }
}

pub(crate) fn cmd_matches_expected(cmd: &str, portable: &str) -> bool {
    let cmd = cmd.trim();
    if cmd == portable.trim() {
        return true;
    }
    if cmd == "sobre" {
        return true;
    }
    // A configured verbatim hook binary ($HOME/.local/bin/sobre for
    // settings synced across machines) is the expected command by definition —
    // never flag it stale, or doctor --fix would rewrite it back to the
    // machine-absolute path and restart the sync ping-pong.
    if let Some(override_cmd) = crate::core::portable_binary::hook_binary_override()
        && cmd == override_cmd.trim()
    {
        return true;
    }
    if let Some(resolved) = resolve_sobre_binary()
        && cmd == resolved.to_string_lossy().trim()
    {
        return true;
    }
    false
}

/// The programs of the `sobre` hook commands in a hook config file, unquoted:
/// every JSON string that is a hook command `<binary> hook <action>`
/// ([`crate::hooks::hook_program`]), so a path with a space reads whole.
pub(crate) fn hook_binary_refs(content: &str) -> Vec<String> {
    json_strings(content)
        .iter()
        .filter(|s| crate::hooks::is_sobre_hook_command(s))
        .filter_map(|s| crate::hooks::hook_program(s).map(str::to_string))
        .collect()
}

/// The string literals of a JSON (or JSONC) text, unescaped; tolerant of
/// anything between them, so it reads a file a parser would refuse.
fn json_strings(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = content.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some(other) => s.push(other),
                    None => {}
                },
                other => s.push(other),
            }
        }
        out.push(s);
    }
    out
}

/// If a hook file references a `sobre` binary path that does not match the
/// currently installed binary (and none of its references do), return that
/// stale path. Returns `None` when there are no hook references or at least one
/// reference points at the current binary (or the bare `sobre` PATH command).
pub(crate) fn stale_hook_binary(content: &str, binary: &str) -> Option<String> {
    let refs = hook_binary_refs(content);
    if refs.is_empty() || refs.iter().any(|r| cmd_matches_expected(r, binary)) {
        return None;
    }
    refs.into_iter().next()
}

/// Cascade hook health for Windsurf. `sobre init` writes observe hooks into
/// `~/.codeium/windsurf/hooks.json`; the `hook observe` command is the
/// marker. A missing file is genuine drift, fixed
/// by re-running setup.
pub(crate) fn check_windsurf_hooks(home: &std::path::Path) -> NamedCheck {
    let path = home.join(".codeium/windsurf/hooks.json");
    let ok = std::fs::read_to_string(&path).is_ok_and(|c| c.contains("hook observe"));
    NamedCheck {
        name: "Cascade hooks".to_string(),
        ok,
        detail: if ok {
            format!("ok ({})", path.display())
        } else {
            "missing (run: sobre init)".to_string()
        },
    }
}

/// Skill is intentionally absent for agents that consume dedicated markdown rules
/// (Windsurf): no `SKILL.md` is installed, by design. Stating it explicitly stops
/// users from treating the absence as a fault.
pub(crate) fn skill_not_applicable_note() -> NamedCheck {
    NamedCheck {
        name: "Skill".to_string(),
        ok: true,
        detail: "N/A — Windsurf has no skills; it gets the MCP instructions".to_string(),
    }
}

/// Most recent `sobre_*` MCP tool call from the usage ledger ("12m ago" /
/// "never"). The clearest signal of whether the agent actually drives
/// sobre through MCP — "never" means the agent is using native tools instead
/// of `sobre_*`, not that sobre is broken. Never fails (informational).
pub(crate) fn last_sobre_call_check() -> NamedCheck {
    let detail = match last_sobre_call_ago() {
        Some(ago) => format!("{ago} (most recent sobre_* MCP call)"),
        None => "never — the agent has not called any sobre_* MCP tool yet".to_string(),
    };
    NamedCheck {
        name: "Last sobre_* call".to_string(),
        ok: true,
        detail,
    }
}

pub(crate) fn last_sobre_call_ago() -> Option<String> {
    use chrono::TimeZone;
    let last = crate::core::usage::load()
        .into_iter()
        .rev()
        .find(|ev| ev.str("sobre.surface") == Some("mcp"))?;
    let at = chrono::Local
        .timestamp_opt(i64::try_from(last.unix_secs()).ok()?, 0)
        .single()?;
    Some(humanize_ago(chrono::Local::now().signed_duration_since(at)))
}

pub(crate) fn humanize_ago(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}
