//! Hook installations (Gemini trust + hooks, Cursor hooks.json, Claude
//! settings hooks).

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn check_gemini_trust_and_hooks(home: &std::path::Path, binary: &str) -> NamedCheck {
    let settings = home.join(".gemini").join("settings.json");
    if !settings.exists() {
        return NamedCheck {
            name: "Gemini hooks".to_string(),
            ok: false,
            detail: format!("missing ({})", settings.display()),
        };
    }
    let content = std::fs::read_to_string(&settings).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "Gemini hooks".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", settings.display()),
        };
    };

    let trust_ok = v
        .get("mcpServers")
        .and_then(|m| m.get("lean-ctx"))
        .and_then(|e| e.get("trust"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);

    let hooks_ok = v
        .get("hooks")
        .and_then(|h| h.get("BeforeTool"))
        .and_then(|x| x.as_array())
        .is_some_and(|arr| {
            let mut saw_rewrite = false;
            let mut saw_redirect_or_deny = false;
            for entry in arr {
                let hooks = entry
                    .get("hooks")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_default();
                for h in hooks {
                    let cmd = h
                        .get("command")
                        .and_then(|c| c.as_str())
                        .unwrap_or_default();
                    let first = cmd.split_whitespace().next().unwrap_or_default();
                    if cmd.contains("hook rewrite") && cmd_matches_expected(first, binary) {
                        saw_rewrite = true;
                    }
                    if (cmd.contains("hook redirect") || cmd.contains("hook deny"))
                        && cmd_matches_expected(first, binary)
                    {
                        saw_redirect_or_deny = true;
                    }
                }
            }
            saw_rewrite && saw_redirect_or_deny
        });

    let scripts_ok = home
        .join(".gemini")
        .join("hooks")
        .join("lean-ctx-rewrite-gemini.sh")
        .exists()
        && home
            .join(".gemini")
            .join("hooks")
            .join("lean-ctx-redirect-gemini.sh")
            .exists();

    let ok = trust_ok && hooks_ok && scripts_ok;
    NamedCheck {
        name: "Gemini hooks".to_string(),
        ok,
        detail: if ok {
            format!("ok ({})", settings.display())
        } else {
            "drift (hooks/trust/scripts)".to_string()
        },
    }
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
    let has_rewrite = pre.iter().any(|e| {
        e.get("matcher").and_then(|m| m.as_str()) == Some("Shell")
            && e.get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.contains(" hook rewrite"))
    });
    let has_redirect_or_deny = pre.iter().any(|e| {
        matches!(
            e.get("matcher").and_then(|m| m.as_str()),
            Some("Read|Grep|Glob" | "Read|Grep" | "Read" | "Grep")
        ) && e
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains(" hook redirect") || c.contains(" hook deny"))
    });
    let entries_ok = has_rewrite && has_redirect_or_deny;
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

/// #719: staleness check for the generated wrapper scripts in
/// `<state>/hooks/`. `settings.json` staleness got covered in #708; the
/// wrappers are the second place a machine-absolute path hides in a synced
/// setup — a wrapper pointing at another machine's install dies at exec time
/// with no surfaced error, so doctor must name it. A portable form
/// (`$HOME/…`) that resolves on this machine is healthy by definition.
pub(crate) fn check_hook_wrapper_scripts(
    hooks_dir: &std::path::Path,
    binary: &str,
    home: &std::path::Path,
) -> NamedCheck {
    let wrapper_names = [
        "lean-ctx-rewrite.sh",
        "lean-ctx-rewrite-native",
        "lean-ctx-redirect-native",
    ];
    let mut stale: Option<String> = None;
    let mut seen = 0usize;
    for name in wrapper_names {
        let path = hooks_dir.join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        seen += 1;
        let Some(token) = crate::hooks::wrapper_binary_token(&content) else {
            continue;
        };
        let ok = super::wiring::cmd_matches_expected(&token, binary)
            || crate::hooks::wrapper_content_is_portable_and_working(&content, home);
        if !ok && stale.is_none() {
            stale = Some(format!("{token} ({name})"));
        }
    }
    let (ok, detail) = if seen == 0 {
        // settings drift already reports a never-ran setup; no double report.
        (true, format!("not installed ({})", hooks_dir.display()))
    } else if let Some(s) = stale {
        (
            false,
            format!("stale binary {s} — run lean-ctx init --repair"),
        )
    } else {
        (true, format!("ok ({seen} wrapper scripts)"))
    };
    NamedCheck {
        name: "Hook wrappers".to_string(),
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
    let pre = v
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let joined = serde_json::to_string(&pre).unwrap_or_default();
    let entries_ok = joined.contains(" hook rewrite")
        && (joined.contains(" hook redirect") || joined.contains(" hook deny"));
    let stale = stale_hook_binary(&joined, binary);
    finalize_hook_check("Hooks", path, entries_ok, stale)
}
