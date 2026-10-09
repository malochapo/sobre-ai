//! Tests for hook installation and path bridging.

#[allow(clippy::wildcard_imports)]
use super::*;

// ── portable hook-binary override for multi-machine synced configs ──

#[test]
fn hook_command_binary_honors_override_and_skips_msys_rewrite() {
    let iso = crate::core::data_dir::isolated_data_dir();
    let cfg = iso.path().join("config.toml");
    std::fs::write(&cfg, "hook_binary = \"$HOME/.local/bin/sobre\"\n").unwrap();
    assert_eq!(resolve_hook_command_binary(), "$HOME/.local/bin/sobre");

    std::fs::remove_file(&cfg).unwrap();
    // Without an override the contract holds: resolved absolute path.
    assert!(std::path::Path::new(&resolve_hook_command_binary()).is_absolute());
    // MCP server entries always keep the absolute path — env-var forms would
    // break hosts that spawn the command without a shell.
    assert!(std::path::Path::new(&resolve_binary_path()).is_absolute());
}

#[test]
fn claude_settings_hooks_emit_override_verbatim_and_stay_idempotent() {
    let iso = crate::core::data_dir::isolated_data_dir();
    std::fs::write(
        iso.path().join("config.toml"),
        "hook_binary = \"$USERPROFILE/AppData/Roaming/npm/node_modules/sobre-bin/bin/sobre.exe\"\n",
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();

    install_claude_hook_config(home.path()).unwrap();

    let settings_path = home.path().join(".claude/settings.json");
    let settings = std::fs::read_to_string(&settings_path).expect("settings.json written");
    assert!(
        settings.contains(
            "$USERPROFILE/AppData/Roaming/npm/node_modules/sobre-bin/bin/sobre.exe hook compress"
        ),
        "hook command must carry the portable form verbatim: {settings}"
    );
    assert!(
        !settings.contains(&resolve_binary_path()),
        "no machine-absolute path may leak into the synced settings.json"
    );

    // Re-running with the same override is a no-op — the sync ping-pong
    // reported came from exactly this rewrite cycle.
    install_claude_hook_config(home.path()).unwrap();
    let after = std::fs::read_to_string(&settings_path).unwrap();
    assert_eq!(settings, after, "idempotent under a stable override");
}

#[test]
fn bash_path_windows_drive_converted() {
    assert_eq!(
        to_bash_compatible_path("C:\\Users\\Fraser\\bin\\sobre.exe"),
        "/c/Users/Fraser/bin/sobre.exe"
    );
}

#[test]
fn bash_path_windows_forward_slashes() {
    assert_eq!(
        to_bash_compatible_path("C:/Users/Fraser/bin/sobre.exe"),
        "/c/Users/Fraser/bin/sobre.exe"
    );
}

#[test]
fn bash_path_bare_name_unchanged() {
    assert_eq!(to_bash_compatible_path("sobre"), "sobre");
}

// MSYS2 drive mapping applies on Windows hosts only — on Linux/macOS
// /c/… is a literal directory and must pass through.
#[cfg(windows)]
#[test]
fn normalize_msys2_path() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("/c/Users/game/Downloads/project"),
        "C:/Users/game/Downloads/project"
    );
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("/d/Projects/app/src"),
        "D:/Projects/app/src"
    );
}

#[cfg(not(windows))]
#[test]
fn normalize_msys2_path_untouched_on_unix() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path_lexical("/c/Users/game/Downloads/project"),
        "/c/Users/game/Downloads/project"
    );
}

#[test]
fn normalize_backslashes() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("C:\\Users\\game\\project\\src"),
        "C:/Users/game/project/src"
    );
}

#[test]
fn normalize_mixed_separators() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("C:\\Users/game\\project/src"),
        "C:/Users/game/project/src"
    );
}

#[test]
fn normalize_double_slashes() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("/home/user//project///src"),
        "/home/user/project/src"
    );
}

#[test]
fn normalize_trailing_slash() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("/home/user/project/"),
        "/home/user/project"
    );
}

#[test]
fn normalize_root_preserved() {
    assert_eq!(crate::core::pathutil::normalize_tool_path("/"), "/");
}

#[test]
fn normalize_windows_root_preserved() {
    assert_eq!(crate::core::pathutil::normalize_tool_path("C:/"), "C:/");
}

#[test]
fn normalize_unix_path_unchanged() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("/home/user/project/src/main.rs"),
        "/home/user/project/src/main.rs"
    );
}

#[test]
fn normalize_relative_path_unchanged() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("src/main.rs"),
        "src/main.rs"
    );
}

#[test]
fn normalize_dot_unchanged() {
    assert_eq!(crate::core::pathutil::normalize_tool_path("."), ".");
}

#[test]
fn normalize_unc_path_preserved() {
    assert_eq!(
        crate::core::pathutil::normalize_tool_path("//server/share/file"),
        "//server/share/file"
    );
}

// Drive translation only applies on Windows hosts.
#[cfg(windows)]
#[test]
fn from_bash_to_native_converts_msys_drive() {
    assert_eq!(
        from_bash_to_native_path("/c/Users/ABC/sobre"),
        "C:/Users/ABC/sobre"
    );
    assert_eq!(
        from_bash_to_native_path("/d/Program Files/sobre.exe"),
        "D:/Program Files/sobre.exe"
    );
}

#[test]
fn from_bash_to_native_bare_name() {
    assert_eq!(from_bash_to_native_path("sobre"), "sobre");
}

// The bash→native return leg only translates on Windows hosts.
#[cfg(windows)]
#[test]
fn roundtrip_windows_path() {
    let native = r"C:\Users\ABC\AppData\Local\sobre\sobre.exe";
    let bash = to_bash_compatible_path(native);
    let back = from_bash_to_native_path(&bash);
    assert_eq!(back, "C:/Users/ABC/AppData/Local/sobre/sobre.exe");
}

#[test]
fn roundtrip_unix_path() {
    let native = "/usr/local/bin/sobre";
    let bash = to_bash_compatible_path(native);
    assert_eq!(bash, native);
    let back = from_bash_to_native_path(&bash);
    assert_eq!(back, native);
}
