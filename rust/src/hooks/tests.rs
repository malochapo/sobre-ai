//! Tests for hook installation, path bridging and MCP registration.

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn refresh_covers_every_hook_capable_agent() {
    // Every hook-capable agent must be in exactly one of the two sets, so a
    // newly added agent can never silently skip the post-update hook refresh.
    for agent in HOOK_CAPABLE_AGENTS {
        let refreshed = REFRESHABLE_HOOK_AGENTS.contains(agent);
        let exempt = REFRESH_EXEMPT_AGENTS.contains(agent);
        assert!(
            refreshed ^ exempt,
            "agent `{agent}` must be either refreshed or explicitly exempt (exactly one)"
        );
    }
}

#[test]
fn refresh_sets_reference_only_hook_capable_agents() {
    for agent in REFRESHABLE_HOOK_AGENTS.iter().chain(REFRESH_EXEMPT_AGENTS) {
        assert!(
            HOOK_CAPABLE_AGENTS.contains(agent),
            "`{agent}` is not a hook-capable agent (stale entry?)"
        );
    }
}

// ── #708: portable hook-binary override for multi-machine synced configs ──

#[test]
fn hook_command_binary_honors_override_and_skips_msys_rewrite() {
    let iso = crate::core::data_dir::isolated_data_dir();
    let cfg = iso.path().join("config.toml");
    std::fs::write(&cfg, "hook_binary = \"$HOME/.local/bin/lean-ctx\"\n").unwrap();
    assert_eq!(resolve_hook_command_binary(), "$HOME/.local/bin/lean-ctx");

    std::fs::remove_file(&cfg).unwrap();
    // Without an override the #367 contract holds: resolved absolute path.
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
        "hook_binary = \"$USERPROFILE/AppData/Roaming/npm/node_modules/lean-ctx-bin/bin/lean-ctx.exe\"\n",
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();

    install_claude_hook_config(home.path());

    let settings_path = home.path().join(".claude/settings.json");
    let settings = std::fs::read_to_string(&settings_path).expect("settings.json written");
    assert!(
        settings.contains(
            "$USERPROFILE/AppData/Roaming/npm/node_modules/lean-ctx-bin/bin/lean-ctx.exe hook compress"
        ),
        "hook command must carry the portable form verbatim: {settings}"
    );
    assert!(
        !settings.contains(&resolve_binary_path()),
        "no machine-absolute path may leak into the synced settings.json"
    );

    // Re-running with the same override is a no-op — the sync ping-pong #708
    // reported came from exactly this rewrite cycle.
    install_claude_hook_config(home.path());
    let after = std::fs::read_to_string(&settings_path).unwrap();
    assert_eq!(settings, after, "idempotent under a stable override");
}

#[test]
fn hooks_installed_for_is_false_without_artifacts() {
    let tmp = unique_tmp_dir("leanctx_refresh_empty");
    for agent in REFRESHABLE_HOOK_AGENTS {
        // `codex` resolves its dir via the global CODEX_HOME-aware resolver
        // (not the passed home), so it cannot be isolated to a temp dir here;
        // its detection is exercised by the marker-content test instead.
        if *agent == "codex" {
            continue;
        }
        assert!(
            !hooks_installed_for(agent, &tmp),
            "`{agent}` should not be detected as installed in an empty home"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn hooks_installed_for_detects_marker_content() {
    let tmp = unique_tmp_dir("leanctx_refresh_marker");
    let hooks = tmp.join(".codeium/windsurf/hooks.json");
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();

    // A foreign hooks.json must not trigger a refresh.
    std::fs::write(&hooks, "{\"hooks\":{}}").unwrap();
    assert!(!hooks_installed_for("windsurf", &tmp));

    // Once it mentions lean-ctx, it is ours and must be refreshed.
    std::fs::write(&hooks, "{\"hooks\":{\"cmd\":\"lean-ctx hook observe\"}}").unwrap();
    assert!(hooks_installed_for("windsurf", &tmp));

    let _ = std::fs::remove_dir_all(&tmp);
}

fn unique_tmp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn bash_path_windows_drive_converted() {
    assert_eq!(
        to_bash_compatible_path("C:\\Users\\Fraser\\bin\\lean-ctx.exe"),
        "/c/Users/Fraser/bin/lean-ctx.exe"
    );
}

#[test]
fn bash_path_windows_forward_slashes() {
    assert_eq!(
        to_bash_compatible_path("C:/Users/Fraser/bin/lean-ctx.exe"),
        "/c/Users/Fraser/bin/lean-ctx.exe"
    );
}

#[test]
fn bash_path_bare_name_unchanged() {
    assert_eq!(to_bash_compatible_path("lean-ctx"), "lean-ctx");
}

// MSYS2 drive mapping applies on Windows hosts only — on Linux/macOS
// /c/… is a literal directory and must pass through (GH #397).
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

// Drive translation only applies on Windows hosts (GH #397).
#[cfg(windows)]
#[test]
fn from_bash_to_native_converts_msys_drive() {
    assert_eq!(
        from_bash_to_native_path("/c/Users/ABC/lean-ctx"),
        "C:/Users/ABC/lean-ctx"
    );
    assert_eq!(
        from_bash_to_native_path("/d/Program Files/lean-ctx.exe"),
        "D:/Program Files/lean-ctx.exe"
    );
}

#[test]
fn from_bash_to_native_bare_name() {
    assert_eq!(from_bash_to_native_path("lean-ctx"), "lean-ctx");
}

// The bash→native return leg only translates on Windows hosts (GH #397).
#[cfg(windows)]
#[test]
fn roundtrip_windows_path() {
    let native = r"C:\Users\ABC\AppData\Local\lean-ctx\lean-ctx.exe";
    let bash = to_bash_compatible_path(native);
    let back = from_bash_to_native_path(&bash);
    assert_eq!(back, "C:/Users/ABC/AppData/Local/lean-ctx/lean-ctx.exe");
}

#[test]
fn roundtrip_unix_path() {
    let native = "/usr/local/bin/lean-ctx";
    let bash = to_bash_compatible_path(native);
    assert_eq!(bash, native);
    let back = from_bash_to_native_path(&bash);
    assert_eq!(back, native);
}
