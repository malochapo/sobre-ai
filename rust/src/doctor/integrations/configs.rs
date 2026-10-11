//! Client-specific config formats (Zed, VS Code, Copilot CLI, OpenCode).

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn check_zed_settings(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return mcp_not_connected("Zed config");
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "Zed config".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };
    let entry = v
        .get("context_servers")
        .and_then(|m| m.get("sobre"))
        .cloned();
    let Some(e) = entry else {
        return mcp_not_connected("Zed config");
    };

    let cmd_ok = e
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| cmd_matches_expected(c, binary));

    NamedCheck {
        name: "Zed config".to_string(),
        ok: cmd_ok,
        detail: if cmd_ok {
            format!("ok ({})", path.display())
        } else {
            format!("drift ({})", path.display())
        },
    }
}

pub(crate) fn check_vscode_mcp(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return mcp_not_connected("VS Code MCP");
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "VS Code MCP".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };
    let Some(e) = v.get("servers").and_then(|m| m.get("sobre")) else {
        return mcp_not_connected("VS Code MCP");
    };

    let ty_ok = e.get("type").and_then(|t| t.as_str()) == Some("stdio");
    let cmd_ok = e
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| cmd_matches_expected(c, binary));

    let ok = ty_ok && cmd_ok;
    NamedCheck {
        name: "VS Code MCP".to_string(),
        ok,
        detail: if ok {
            format!("ok ({})", path.display())
        } else {
            format!("drift ({})", path.display())
        },
    }
}

pub(crate) fn check_copilot_cli_mcp(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return mcp_not_connected("Copilot CLI MCP");
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "Copilot CLI MCP".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };
    let Some(e) = v.get("mcpServers").and_then(|m| m.get("sobre")) else {
        return mcp_not_connected("Copilot CLI MCP");
    };

    let cmd_ok = e
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| cmd_matches_expected(c, binary));

    let ok = cmd_ok;
    NamedCheck {
        name: "Copilot CLI MCP".to_string(),
        ok,
        detail: if ok {
            format!("ok ({})", path.display())
        } else {
            format!("drift ({})", path.display())
        },
    }
}

pub(crate) fn check_opencode_config(path: &std::path::Path, binary: &str) -> NamedCheck {
    if !path.exists() {
        return mcp_not_connected("OpenCode MCP");
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = crate::core::jsonc::parse_jsonc(&content).ok();
    let Some(v) = parsed else {
        return NamedCheck {
            name: "OpenCode MCP".to_string(),
            ok: false,
            detail: format!("invalid JSON ({})", path.display()),
        };
    };
    let Some(e) = v.get("mcp").and_then(|m| m.get("sobre")) else {
        return mcp_not_connected("OpenCode MCP");
    };

    let cmd = e
        .get("command")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.as_str());
    let cmd_ok = cmd.is_some_and(|c| cmd_matches_expected(c, binary));
    let ok = cmd_ok;
    NamedCheck {
        name: "OpenCode MCP".to_string(),
        ok,
        detail: if ok {
            format!("ok ({})", path.display())
        } else {
            format!("drift ({})", path.display())
        },
    }
}
