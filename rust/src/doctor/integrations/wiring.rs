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

    let entry = v
        .get("mcpServers")
        .and_then(|m| m.get("lean-ctx"))
        .cloned()
        .or_else(|| {
            v.get("mcp")
                .and_then(|m| m.get("servers"))
                .and_then(|m| m.get("lean-ctx"))
                .cloned()
        });

    let Some(e) = entry else {
        return NamedCheck {
            name: "MCP config".to_string(),
            ok: false,
            detail: format!("lean-ctx missing ({})", path.display()),
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
    if cmd == "lean-ctx" {
        return true;
    }
    // #708: a configured verbatim hook binary ($HOME/.local/bin/lean-ctx for
    // settings synced across machines) is the expected command by definition —
    // never flag it stale, or doctor --fix would rewrite it back to the
    // machine-absolute path and restart the sync ping-pong.
    if let Some(override_cmd) = crate::core::portable_binary::hook_binary_override()
        && cmd == override_cmd.trim()
    {
        return true;
    }
    if let Some(resolved) = resolve_lean_ctx_binary()
        && cmd == resolved.to_string_lossy().trim()
    {
        return true;
    }
    false
}

/// Collect the `lean-ctx` binary tokens that appear immediately before a
/// ` hook ` invocation inside a hook config file. Managed hook commands look
/// like `"<binary> hook compress"` / `"<binary> hook observe"`, so the token
/// directly preceding a
/// ` hook ` delimiter is the binary the hook will execute.
pub(crate) fn hook_binary_refs(content: &str) -> Vec<String> {
    let pieces: Vec<&str> = content.split(" hook ").collect();
    if pieces.len() < 2 {
        return Vec::new();
    }
    pieces[..pieces.len() - 1]
        .iter()
        .filter_map(|piece| {
            // The binary token is the trailing run before " hook ", bounded by
            // whitespace or JSON string delimiters. Splitting on whitespace
            // alone breaks on minified JSON (e.g. `serde_json::to_string`
            // output), where there is no space between the opening quote and
            // the command — we would otherwise capture the whole JSON prefix.
            piece
                .rsplit(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '`')
                .find(|tok| !tok.is_empty())
                .map(|tok| tok.trim_end_matches(',').to_string())
        })
        .filter(|tok| tok.contains("lean-ctx"))
        .collect()
}

/// If a hook file references a `lean-ctx` binary path that does not match the
/// currently installed binary (and none of its references do), return that
/// stale path. Returns `None` when there are no hook references or at least one
/// reference points at the current binary (or the bare `lean-ctx` PATH command).
pub(crate) fn stale_hook_binary(content: &str, binary: &str) -> Option<String> {
    let refs = hook_binary_refs(content);
    if refs.is_empty() || refs.iter().any(|r| cmd_matches_expected(r, binary)) {
        return None;
    }
    refs.into_iter().next()
}

/// Cascade hook health for Windsurf (#593). `lean-ctx init` writes observe +
/// pre_mcp_tool_use hooks into `~/.codeium/windsurf/hooks.json`; the `hook
/// observe` command is the stable marker. A missing file is genuine drift, fixed
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
            "missing (run: lean-ctx init)".to_string()
        },
    }
}

/// Skill is intentionally absent for agents that consume dedicated markdown rules
/// (Windsurf): no `SKILL.md` is installed, by design. Stating it explicitly stops
/// users from treating the absence as a fault (#593).
pub(crate) fn skill_not_applicable_note() -> NamedCheck {
    NamedCheck {
        name: "Skill".to_string(),
        ok: true,
        detail: "N/A — Windsurf has no skills; it gets the MCP instructions".to_string(),
    }
}

/// Most recent real `ctx_*` MCP tool call from the event log ("12m ago" /
/// "never"). #593: the clearest signal of whether the agent actually drives
/// lean-ctx through MCP — an empty `watch` plus "never" means the agent is using
/// native tools instead of `ctx_*`, not that lean-ctx is broken. Never fails
/// (informational), and reads the live event log so it is naturally dynamic.
pub(crate) fn last_ctx_call_check() -> NamedCheck {
    let detail = match last_ctx_call_ago() {
        Some(ago) => format!("{ago} (most recent ctx_* MCP call)"),
        None => "never — the agent has not called any ctx_* MCP tool yet".to_string(),
    };
    NamedCheck {
        name: "Last ctx_* call".to_string(),
        ok: true,
        detail,
    }
}

pub(crate) fn last_ctx_call_ago() -> Option<String> {
    let path = crate::core::paths::state_dir().ok()?.join("events.jsonl");
    let content = std::fs::read_to_string(&path).ok()?;
    let ts = content
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<crate::core::events::LeanCtxEvent>(l).ok())
        .find_map(|ev| {
            let crate::core::events::EventKind::ToolCall { tool, .. } = ev.kind;
            tool.starts_with("ctx_").then_some(ev.timestamp)
        })?;
    let parsed = chrono::NaiveDateTime::parse_from_str(&ts, "%Y-%m-%dT%H:%M:%S%.3f").ok()?;
    let delta = chrono::Local::now()
        .naive_local()
        .signed_duration_since(parsed);
    Some(humanize_ago(delta))
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
