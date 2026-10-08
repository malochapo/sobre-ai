// Auto-split from the former monolithic doctor/mod.rs.

use super::{BOLD, DIM, GREEN, Outcome, RED, RST, WHITE};
use std::path::PathBuf;

/// Human-readable byte size for doctor output (MB / KB / B).
pub(super) fn human_bytes(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Abbreviate the user's `$HOME` to `~` wherever it appears in `text` (#437).
/// Doctor output otherwise mixes `/home/<user>/…` and `~/…`, which is noisy and
/// forces users to redact their username before pasting a report. Matches both
/// the native and the forward-slash spelling of the home prefix; non-home
/// absolute paths (e.g. `/usr/local/bin`) are left untouched.
pub(super) fn tildify_home(text: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return text.to_string();
    };
    let home_str = home.to_string_lossy();
    let home_trimmed = home_str.trim_end_matches(['/', '\\']);
    if home_trimmed.is_empty() {
        return text.to_string();
    }
    let mut out = text.replace(home_trimmed, "~");
    let home_fwd = crate::core::protocol::display_path(home_trimmed);
    if home_fwd != home_trimmed {
        out = out.replace(&home_fwd, "~");
    }
    out
}

/// Render a single path for doctor output: `~` for `$HOME`, forward slashes, and
/// the home prefix matched only on a component boundary (so `/home/foo` never
/// turns `/home/foobar` into `~bar`). (#437)
pub(super) fn display_user_path(path: &std::path::Path) -> String {
    let normalized = crate::core::protocol::display_path(&path.to_string_lossy());
    let Some(home) = dirs::home_dir() else {
        return normalized;
    };
    let home_norm = crate::core::protocol::display_path(&home.to_string_lossy());
    let home_trimmed = home_norm.trim_end_matches('/');
    if home_trimmed.is_empty() {
        return normalized;
    }
    if normalized == home_trimmed {
        return "~".to_string();
    }
    match normalized.strip_prefix(home_trimmed) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => normalized,
    }
}

pub(super) fn print_check(outcome: &Outcome) {
    let mark = if outcome.ok {
        format!("{GREEN}✓{RST}")
    } else {
        format!("{RED}✗{RST}")
    };
    println!("  {mark}  {}", tildify_home(&outcome.line));
}

pub(super) fn path_in_path_env() -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join("sobre").is_file() {
                return true;
            }
            if cfg!(windows) && dir.join("sobre.exe").is_file() {
                return true;
            }
        }
    }
    false
}

pub(super) fn resolve_sobre_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if cfg!(windows) {
                let exe = dir.join("sobre.exe");
                if exe.is_file() {
                    return Some(exe);
                }
            } else {
                let bin = dir.join("sobre");
                if bin.is_file() {
                    return Some(bin);
                }
            }
        }
    }
    None
}

pub(super) fn sobre_version_from_path() -> Outcome {
    let resolved = resolve_sobre_binary();
    let bin = resolved
        .clone()
        .unwrap_or_else(|| std::env::current_exe().unwrap_or_else(|_| "sobre".into()));

    let v = env!("CARGO_PKG_VERSION");
    let note = match std::env::current_exe() {
        Ok(exe) if exe == bin => format!("{DIM}(this binary){RST}"),
        Ok(_) | Err(_) => format!("{DIM}(resolved: {}){RST}", bin.display()),
    };
    Outcome {
        ok: true,
        line: format!("{BOLD}sobre version{RST}  {WHITE}sobre {v}{RST}  {note}"),
    }
}

pub(super) struct McpLocation {
    pub(super) name: &'static str,
    pub(super) display: String,
    pub(super) path: PathBuf,
}

pub(super) fn mcp_config_locations(home: &std::path::Path) -> Vec<McpLocation> {
    let mut locations = vec![
        McpLocation {
            name: "Cursor",
            display: "~/.cursor/mcp.json".into(),
            path: home.join(".cursor").join("mcp.json"),
        },
        McpLocation {
            name: "Claude Code",
            display: format!(
                "{}",
                crate::core::editor_registry::claude_mcp_json_path(home).display()
            ),
            path: crate::core::editor_registry::claude_mcp_json_path(home),
        },
        McpLocation {
            name: "Windsurf",
            display: "~/.codeium/windsurf/mcp_config.json".into(),
            path: home
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json"),
        },
        McpLocation {
            name: "Codex",
            display: {
                let codex_dir =
                    crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
                format!("{}/config.toml", codex_dir.display())
            },
            path: crate::core::home::resolve_codex_dir()
                .unwrap_or_else(|| home.join(".codex"))
                .join("config.toml"),
        },
        McpLocation {
            name: "Gemini CLI",
            display: "~/.gemini/settings.json".into(),
            path: home.join(".gemini").join("settings.json"),
        },
    ];

    #[cfg(unix)]
    {
        let zed_cfg = home.join(".config").join("zed").join("settings.json");
        locations.push(McpLocation {
            name: "Zed",
            display: "~/.config/zed/settings.json".into(),
            path: zed_cfg,
        });
    }

    {
        let path = crate::core::editor_registry::opencode_config_path(home);
        locations.push(McpLocation {
            name: "OpenCode",
            display: path.display().to_string(),
            path,
        });
    }

    #[cfg(target_os = "macos")]
    {
        let vscode_mcp = home.join("Library/Application Support/Code/User/mcp.json");
        locations.push(McpLocation {
            name: "VS Code",
            display: "~/Library/Application Support/Code/User/mcp.json".into(),
            path: vscode_mcp,
        });
    }
    #[cfg(target_os = "linux")]
    {
        let user_dirs = [
            home.join(".config/Code/User"),
            home.join(".config/Code - Insiders/User"),
            home.join(".vscode-server/data/User"),
        ];
        let user_dir = user_dirs
            .iter()
            .find(|p| p.exists())
            .cloned()
            .unwrap_or_else(|| user_dirs[0].clone());
        let vscode_mcp = user_dir.join("mcp.json");
        let display = vscode_mcp.strip_prefix(home).unwrap_or(&vscode_mcp);
        let display_str = format!("~/{}", display.display());
        locations.push(McpLocation {
            name: "VS Code",
            display: display_str,
            path: vscode_mcp,
        });
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let vscode_mcp = std::path::PathBuf::from(&appdata).join("Code/User/mcp.json");
            locations.push(McpLocation {
                name: "VS Code",
                display: "%APPDATA%/Code/User/mcp.json".into(),
                path: vscode_mcp,
            });
        }
    }

    locations.push(McpLocation {
        name: "Copilot CLI",
        display: "~/.copilot/mcp-config.json".into(),
        path: home.join(".copilot/mcp-config.json"),
    });

    locations
}

pub(super) fn has_sobre_mcp_entry(content: &str) -> bool {
    // Parse as JSONC: editor config files (VS Code settings.json / mcp.json,
    // Cursor, Windsurf, …) commonly contain comments and trailing commas which
    // strict JSON rejects. See issue #311.
    if let Ok(json) = crate::core::jsonc::parse_jsonc(content) {
        // Known container keys across editors that hold a map of MCP servers:
        //   mcpServers       — most agents (Cursor, Claude, Windsurf, …)
        //   servers          — VS Code mcp.json
        //   context_servers  — Zed settings.json
        for key in ["mcpServers", "servers", "context_servers"] {
            if let Some(servers) = json.get(key).and_then(|v| v.as_object())
                && servers.contains_key("sobre")
            {
                return true;
            }
        }
        // mcp.sobre — OpenCode's schema (https://opencode.ai/config.json)
        // nests servers DIRECTLY under "mcp", no "servers" level (GH #686).
        if let Some(servers) = json.get("mcp").and_then(|v| v.as_object())
            && servers.contains_key("sobre")
        {
            return true;
        }
        // Parsed cleanly but no sobre entry under any known key.
        return false;
    }
    // Unparseable even as JSONC: fall back to a substring heuristic.
    content.contains("sobre")
}

pub(super) fn sobre_mcp_location_names(
    home: &std::path::Path,
) -> std::collections::BTreeSet<&'static str> {
    let mut names = std::collections::BTreeSet::new();
    for loc in mcp_config_locations(home) {
        if let Ok(content) = std::fs::read_to_string(&loc.path)
            && has_sobre_mcp_entry(&content)
        {
            names.insert(loc.name);
        }
    }
    names
}

pub(super) fn proxy_auth_probe(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let token = crate::core::session_token::resolve_proxy_token("SOBRE_PROXY_TOKEN");

    let Ok(mut stream) = TcpStream::connect_timeout(&addr, crate::proxy_setup::proxy_timeout())
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));

    let req = format!(
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }

    let mut buf = [0u8; 128];
    let Ok(n) = stream.read(&mut buf) else {
        return false;
    };
    let response = String::from_utf8_lossy(&buf[..n]);
    response.contains("200") || response.contains("ok")
}

/// Fetches the proxy's live upstreams from the authenticated `/status` endpoint.
/// Returns `(anthropic, openai)`, or `None` if unreachable or
/// the response lacks the `upstreams` block. Drives the #449 drift check.
pub(super) fn proxy_live_upstreams(port: u16) -> Option<(String, String)> {
    let token = crate::core::session_token::resolve_proxy_token("SOBRE_PROXY_TOKEN");
    let url = format!("http://127.0.0.1:{port}/status");
    let resp = ureq::get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()?;
    let body = resp.into_body().read_to_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let up = v.get("upstreams")?;
    Some((
        up.get("anthropic")?.as_str()?.to_string(),
        up.get("openai")?.as_str()?.to_string(),
    ))
}

pub(super) fn claude_binary_exists() -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("which")
            .arg("claude")
            .output()
            .is_ok_and(|o| o.status.success())
    }
    #[cfg(windows)]
    {
        std::process::Command::new("where")
            .arg("claude")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::{display_user_path, has_sobre_mcp_entry, tildify_home};
    use std::path::Path;

    // GH #686: OpenCode's schema (https://opencode.ai/config.json) nests
    // servers DIRECTLY under "mcp" — `mcp.sobre`, no "servers" level.
    #[test]
    fn opencode_direct_mcp_child_is_recognized() {
        let opencode = r#"{
            "$schema": "https://opencode.ai/config.json",
            "mcp": {
                "sobre": { "command": ["/usr/bin/sobre"], "enabled": true, "type": "local" }
            }
        }"#;
        assert!(has_sobre_mcp_entry(opencode));
    }

    #[test]
    fn mcp_servers_form_recognized() {
        let flat = r#"{ "mcpServers": { "sobre": { "command": "sobre" } } }"#;
        assert!(has_sobre_mcp_entry(flat));
    }

    #[test]
    fn unrelated_mcp_children_are_not_a_match() {
        // A different server directly under "mcp" must not count as ours.
        let other = r#"{ "mcp": { "some-other-server": { "command": "x" } } }"#;
        assert!(!has_sobre_mcp_entry(other));
    }

    #[test]
    fn display_user_path_abbreviates_home() {
        if let Some(home) = dirs::home_dir() {
            let shown = display_user_path(&home.join(".cursor").join("mcp.json"));
            assert_eq!(shown, "~/.cursor/mcp.json");
            assert_eq!(display_user_path(&home), "~");
        }
    }

    #[test]
    fn display_user_path_leaves_non_home_paths() {
        assert_eq!(
            display_user_path(Path::new("/opt/sobre/bin")),
            "/opt/sobre/bin"
        );
    }

    #[test]
    fn display_user_path_normalizes_separators() {
        assert_eq!(
            display_user_path(Path::new("rel\\sub\\file")),
            "rel/sub/file"
        );
    }

    #[test]
    fn display_user_path_respects_component_boundary() {
        // `/home/foo` must never turn `/home/foo-sibling/...` into `~-sibling/...`.
        if let Some(home) = dirs::home_dir()
            && let (Some(parent), Some(name)) = (home.parent(), home.file_name())
        {
            let sibling = parent
                .join(format!("{}-sibling", name.to_string_lossy()))
                .join("x");
            assert!(!display_user_path(&sibling).starts_with('~'));
        }
    }

    #[test]
    fn tildify_home_replaces_home_in_formatted_text() {
        if let Some(home) = dirs::home_dir() {
            let line = format!("ok ({}/.config/x)", home.display());
            let shown = tildify_home(&line);
            assert!(shown.contains("~/.config/x"), "got: {shown}");
            assert!(
                !shown.contains(&home.display().to_string()),
                "home leaked: {shown}"
            );
        }
    }
}
