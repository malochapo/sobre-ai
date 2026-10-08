//! Codex CLI/Desktop: config.toml entry, history visibility, proxy
//! entry classification, notify hooks.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn check_codex_toml(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return NamedCheck {
            name: "Codex MCP".to_string(),
            ok: false,
            detail: format!("missing ({})", path.display()),
        };
    }
    let content = read_effective_codex_config(path);
    let parsed: Result<toml::Value, _> = toml::from_str(&content);
    let Ok(v) = parsed else {
        return NamedCheck {
            name: "Codex MCP".to_string(),
            ok: false,
            detail: format!("invalid TOML ({})", path.display()),
        };
    };
    let cmd = v
        .get("mcp_servers")
        .and_then(|t| t.get("sobre"))
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str());
    let ok = cmd.is_some_and(|c| cmd_matches_expected(c, binary));
    NamedCheck {
        name: "Codex MCP".to_string(),
        ok,
        detail: if ok {
            format!("ok ({})", path.display())
        } else {
            format!("drift ({})", path.display())
        },
    }
}

fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    if let (Some(base), Some(overlay)) = (base.as_table_mut(), overlay.as_table()) {
        for (key, value) in overlay {
            if let Some(existing) = base.get_mut(key) {
                merge_toml(existing, value.clone());
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = overlay;
    }
}

fn read_effective_codex_config(path: &std::path::Path) -> String {
    let Some(parent) = path.parent() else {
        return std::fs::read_to_string(path).unwrap_or_default();
    };
    let base_path = parent.join("config.toml");
    if path == base_path {
        return std::fs::read_to_string(path).unwrap_or_default();
    }

    let Ok(base) = std::fs::read_to_string(&base_path) else {
        return std::fs::read_to_string(path).unwrap_or_default();
    };
    let Ok(overlay) = std::fs::read_to_string(path) else {
        return base;
    };
    let Ok(mut base_value) = toml::from_str::<toml::Value>(&base) else {
        return overlay;
    };
    let Ok(overlay_value) = toml::from_str::<toml::Value>(&overlay) else {
        return base;
    };
    merge_toml(&mut base_value, overlay_value);
    toml::to_string(&base_value).unwrap_or(base)
}

pub(crate) fn check_codex_hooks_enabled(home: &std::path::Path) -> NamedCheck {
    let path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| home.join(".codex/config.toml"));
    if !path.exists() {
        return NamedCheck {
            name: "Codex hooks".to_string(),
            ok: false,
            detail: format!("missing ({})", path.display()),
        };
    }
    let content = read_effective_codex_config(&path);
    let parsed: Result<toml::Value, _> = toml::from_str(&content);
    let Ok(v) = parsed else {
        return NamedCheck {
            name: "Codex hooks".to_string(),
            ok: false,
            detail: format!("invalid TOML ({})", path.display()),
        };
    };
    let features = v.get("features");
    let ok = features
        .and_then(|t| t.get("hooks"))
        .and_then(toml::Value::as_bool)
        == Some(true)
        || features
            .and_then(|t| t.get("codex_hooks"))
            .and_then(toml::Value::as_bool)
            == Some(true);
    NamedCheck {
        name: "Codex hooks".to_string(),
        ok,
        detail: if ok {
            format!("enabled ({})", path.display())
        } else {
            format!("disabled ({})", path.display())
        },
    }
}

/// Informational note (always `ok`): sobre's transparent shell/file
/// compression is hook-driven, and whether Codex lifecycle hooks fire depends on
/// the surface (CLI / Desktop / Cloud), the Codex version, and whether the hooks
/// are trusted (`/hooks`). Rather than asserting any one surface "can't" run hooks
/// (it varies and changes across Codex releases), this note points at the reliable
/// path: the sobre MCP tools (`ctx_shell`/`ctx_read`/`ctx_search`) compress on
/// every surface. Guidance only — it never fails.
pub(crate) fn codex_desktop_note() -> NamedCheck {
    NamedCheck {
        name: "Codex compression".to_string(),
        ok: true,
        detail: "hooks auto-compress when trusted (/hooks); the ctx_shell/ctx_read/ctx_search MCP tools compress reliably on every surface (CLI/Desktop/Cloud)".to_string(),
    }
}

pub(crate) fn check_codex_hooks_json(home: &std::path::Path, binary: &str) -> NamedCheck {
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    let path = codex_dir.join("hooks.json");
    if !path.exists() {
        return NamedCheck {
            name: "Codex hooks.json".to_string(),
            ok: false,
            detail: format!("missing ({})", path.display()),
        };
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "Codex hooks.json".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };
    // sobre installs only observe hooks for Codex: Codex accepts a rewritten
    // command only with an `allow` decision, which sobre never emits.
    let entries_ok = v.get("hooks").is_some_and(|h| {
        serde_json::to_string(h)
            .unwrap_or_default()
            .contains(" hook observe")
    });
    let stale = stale_hook_binary(&content, binary);
    let ok = entries_ok && stale.is_none();
    let detail = if !entries_ok {
        format!("missing managed entries ({})", path.display())
    } else if let Some(old) = stale {
        format!("stale binary {old} — run sobre init --repair")
    } else {
        format!("ok ({})", path.display())
    };
    NamedCheck {
        name: "Codex hooks.json".to_string(),
        ok,
        detail,
    }
}
