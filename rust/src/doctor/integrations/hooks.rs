//! Hook installations (Gemini settings hooks, Cursor hooks.json, Claude
//! settings hooks).

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn check_gemini_hooks(home: &std::path::Path, binary: &str) -> NamedCheck {
    let settings = home.join(".gemini").join("settings.json");
    if !settings.exists() {
        return NamedCheck {
            name: "Gemini hooks".to_string(),
            ok: false,
            detail: format!("missing ({})", settings.display()),
        };
    }
    let content = std::fs::read_to_string(&settings).unwrap_or_default();
    let Ok(v) = crate::core::jsonc::parse_jsonc(&content) else {
        return NamedCheck {
            name: "Gemini hooks".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", settings.display()),
        };
    };
    let before = v
        .get("hooks")
        .and_then(|h| h.get("BeforeTool"))
        .cloned()
        .unwrap_or_default();
    let joined = serde_json::to_string(&before).unwrap_or_default();
    let entries_ok = joined.contains(" hook gemini");
    let stale = stale_hook_binary(&joined, binary);
    finalize_hook_check("Gemini hooks", &settings, entries_ok, stale)
}

pub(crate) fn check_cursor_hooks(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return NamedCheck {
            name: "Hooks".to_string(),
            ok: false,
            detail: format!("missing ({})", path.display()),
        };
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "Hooks".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };
    let pre = v
        .get("hooks")
        .and_then(|h| h.get("preToolUse"))
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let entries_ok = pre.iter().any(|e| {
        e.get("matcher").and_then(|m| m.as_str()) == Some("Shell")
            && e.get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.contains(" hook cursor"))
    });
    let stale = stale_hook_binary(&content, binary);
    finalize_hook_check("Hooks", path, entries_ok, stale)
}

/// Shared verdict for hook checks: distinguishes missing/incomplete managed
/// entries from a stale binary reference, so `doctor` can show the precise
/// repair reason (the #249 observability pattern, extended to hook staleness).
pub(crate) fn finalize_hook_check(
    name: &str,
    path: &std::path::Path,
    entries_ok: bool,
    stale: Option<String>,
) -> NamedCheck {
    let ok = entries_ok && stale.is_none();
    let detail = if !entries_ok {
        format!("drift ({})", path.display())
    } else if let Some(old) = stale {
        format!("stale binary {old} — run lean-ctx init --repair")
    } else {
        format!("ok ({})", path.display())
    };
    NamedCheck {
        name: name.to_string(),
        ok,
        detail,
    }
}

pub(crate) fn check_claude_hooks(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return NamedCheck {
            name: "Hooks".to_string(),
            ok: false,
            detail: format!("missing ({})", path.display()),
        };
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "Hooks".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };
    let post = v
        .get("hooks")
        .and_then(|h| h.get("PostToolUse"))
        .cloned()
        .unwrap_or_default();
    let joined = serde_json::to_string(&post).unwrap_or_default();
    let entries_ok = joined.contains(" hook compress");
    let stale = stale_hook_binary(&joined, binary);
    finalize_hook_check("Hooks", path, entries_ok, stale)
}
