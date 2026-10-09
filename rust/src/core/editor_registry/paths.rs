use std::path::{Path, PathBuf};

/// OpenCode's global `opencode.json`, in `~/.config/opencode` on every OS
/// (OpenCode resolves it with xdg-basedir, Windows included), next to the
/// plugin and the skill. The MCP entry and the proxy wiring both live in it.
pub fn opencode_config_path(home: &Path) -> PathBuf {
    home.join(".config/opencode/opencode.json")
}

/// Zed's `settings.json`, which holds its `context_servers`.
pub fn zed_settings_path(home: &Path) -> PathBuf {
    zed_config_dir(home).join("settings.json")
}

/// Zed's config directory: `~/.config/zed` on macOS, `$XDG_CONFIG_HOME/zed`
/// (default `~/.config/zed`) on Linux, `%APPDATA%\Zed` on Windows.
pub fn zed_config_dir(home: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("Zed");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        && xdg.is_absolute()
    {
        return xdg.join("zed");
    }
    home.join(".config/zed")
}

pub fn vscode_mcp_path() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        #[cfg(target_os = "macos")]
        {
            return home.join("Library/Application Support/Code/User/mcp.json");
        }
        #[cfg(target_os = "linux")]
        {
            return resolve_vscode_global_storage(&home, "User/mcp.json");
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                return PathBuf::from(appdata).join("Code/User/mcp.json");
            }
        }
        #[allow(unreachable_code)]
        home.join(".config/Code/User/mcp.json")
    } else {
        PathBuf::from("/nonexistent")
    }
}

/// Resolves the correct VS Code-family base directory on Linux.
/// Checks VSCodium, Code - OSS, Code, Code - Insiders, then the dev-container
/// server dir (in that order) — returns the first existing path, falling back
/// to the standard `Code` path for fresh installs. Most-specific first: a
/// VSCodium user with a leftover `~/.config/Code` (one accidental Code launch
/// is enough) must keep resolving to VSCodium. Dev containers use
/// `.vscode-server/data` instead of `.config`, so we check both.
#[cfg(target_os = "linux")]
fn resolve_vscode_global_storage(home: &Path, suffix: &str) -> PathBuf {
    const CANDIDATES: &[&str] = &[
        ".config/VSCodium",
        ".config/Code - OSS",
        ".config/Code",
        ".config/Code - Insiders",
        ".vscode-server/data",
    ];
    for base in CANDIDATES {
        let path = home.join(base);
        if path.exists() {
            return path.join(suffix);
        }
    }
    home.join(".config/Code").join(suffix)
}

pub fn claude_mcp_json_path(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir).join(".claude.json");
        }
    }
    home.join(".claude.json")
}

pub fn claude_state_dir(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home.join(".claude")
}
