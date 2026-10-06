//! Tests for hook installation, path bridging and MCP registration.

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn refresh_covers_every_hybrid_agent() {
    // Every Hybrid agent must be in exactly one of the two sets, so a newly
    // added agent can never silently skip the post-update hook refresh.
    for agent in HYBRID_AGENTS {
        let refreshed = REFRESHABLE_HOOK_AGENTS.contains(agent);
        let exempt = REFRESH_EXEMPT_HYBRID_AGENTS.contains(agent);
        assert!(
            refreshed ^ exempt,
            "hybrid agent `{agent}` must be either refreshed or explicitly exempt (exactly one)"
        );
    }
}

#[test]
fn refresh_sets_reference_only_hybrid_agents() {
    for agent in REFRESHABLE_HOOK_AGENTS {
        assert!(
            HYBRID_AGENTS.contains(agent),
            "refreshable agent `{agent}` is not a Hybrid agent"
        );
    }
    for agent in REFRESH_EXEMPT_HYBRID_AGENTS {
        assert!(
            HYBRID_AGENTS.contains(agent),
            "exempt agent `{agent}` is not a Hybrid agent (stale exemption?)"
        );
    }
}

// ── #555: .github/copilot-instructions.md ──────────────────────────────

#[test]
fn copilot_instructions_created_with_lean_ctx_block() {
    let _iso = crate::core::data_dir::isolated_data_dir();
    let tmp = tempfile::tempdir().unwrap();
    ensure_copilot_instructions(tmp.path());

    let path = tmp.path().join(".github/copilot-instructions.md");
    let content = std::fs::read_to_string(&path).expect("copilot-instructions.md created");
    assert!(content.contains(crate::core::rules_canonical::START_MARK));
    assert!(content.contains(crate::core::rules_canonical::END_MARK));
    assert!(content.contains("lean-ctx"));
}

#[test]
fn copilot_instructions_idempotent() {
    let _iso = crate::core::data_dir::isolated_data_dir();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".github/copilot-instructions.md");

    ensure_copilot_instructions(tmp.path());
    let first = std::fs::read_to_string(&path).unwrap();
    ensure_copilot_instructions(tmp.path());
    let second = std::fs::read_to_string(&path).unwrap();

    assert_eq!(first, second, "re-running must produce identical bytes");
    assert_eq!(
        first
            .matches(crate::core::rules_canonical::START_MARK)
            .count(),
        1,
        "exactly one lean-ctx block, no duplication"
    );
}

#[test]
fn copilot_instructions_preserve_user_content() {
    let _iso = crate::core::data_dir::isolated_data_dir();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".github/copilot-instructions.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# House rules\n\nAlways write tests.\n").unwrap();

    ensure_copilot_instructions(tmp.path());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("# House rules"));
    assert!(content.contains("Always write tests."));
    assert!(content.contains(crate::core::rules_canonical::START_MARK));

    // Idempotent on a user-authored file as well.
    ensure_copilot_instructions(tmp.path());
    let again = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, again);
    assert_eq!(
        again
            .matches(crate::core::rules_canonical::START_MARK)
            .count(),
        1
    );
}

#[test]
fn copilot_instructions_block_is_removable() {
    // Mirrors the uninstall path: the marked block must be strippable while
    // user content survives.
    let _iso = crate::core::data_dir::isolated_data_dir();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".github/copilot-instructions.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# House rules\n\nKeep it tidy.\n").unwrap();
    ensure_copilot_instructions(tmp.path());

    let content = std::fs::read_to_string(&path).unwrap();
    let cleaned = crate::marked_block::remove_content(
        &content,
        crate::core::rules_canonical::START_MARK,
        crate::core::rules_canonical::END_MARK,
    );
    assert!(!cleaned.contains(crate::core::rules_canonical::START_MARK));
    assert!(cleaned.contains("# House rules"));
}

#[test]
fn vscode_instruction_setting_set_when_absent_and_preserves() {
    let tmp = tempfile::tempdir().unwrap();
    let vscode = tmp.path().join(".vscode");
    std::fs::create_dir_all(&vscode).unwrap();
    let settings = vscode.join("settings.json");
    std::fs::write(&settings, "{\n  \"editor.fontSize\": 13\n}\n").unwrap();

    ensure_vscode_instruction_files_setting(tmp.path());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["editor.fontSize"], 13);
    assert_eq!(
        v["github.copilot.chat.codeGeneration.useInstructionFiles"],
        true
    );
}

#[test]
fn vscode_instruction_setting_respects_explicit_user_value() {
    let tmp = tempfile::tempdir().unwrap();
    let vscode = tmp.path().join(".vscode");
    std::fs::create_dir_all(&vscode).unwrap();
    let settings = vscode.join("settings.json");
    std::fs::write(
        &settings,
        "{\n  \"github.copilot.chat.codeGeneration.useInstructionFiles\": false\n}\n",
    )
    .unwrap();

    ensure_vscode_instruction_files_setting(tmp.path());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        v["github.copilot.chat.codeGeneration.useInstructionFiles"], false,
        "an explicit user value must not be overridden"
    );
}

#[test]
fn mcp_env_pairs_propagate_project_root_from_env() {
    // #403: init must bake the captured project scope into the MCP server
    // entry, otherwise the long-lived server loses it.
    let _iso = crate::core::data_dir::isolated_data_dir();
    crate::test_env::set_var("LEAN_CTX_PROJECT_ROOT", "/work/main");

    let pairs = mcp_server_env_pairs();
    let get = |k: &str| pairs.iter().find(|(p, _)| p == k).map(|(_, v)| v.as_str());
    assert!(
        get("LEAN_CTX_DATA_DIR").is_none(),
        "data dir is auto-detected at runtime, never pinned into the config (GH #408)"
    );
    assert_eq!(get("LEAN_CTX_PROJECT_ROOT"), Some("/work/main"));

    // The JSON view mirrors the pairs for the JSON-config agents.
    let json = mcp_server_env_json();
    assert_eq!(json["LEAN_CTX_PROJECT_ROOT"].as_str(), Some("/work/main"));

    crate::test_env::remove_var("LEAN_CTX_PROJECT_ROOT");
}

#[test]
fn mcp_env_pairs_omit_roots_when_unset() {
    // No project context configured anywhere ⇒ no env vars are emitted: the
    // data dir is auto-detected (never pinned, GH #408) and we never write
    // empty/placeholder root keys into agent configs.
    let _iso = crate::core::data_dir::isolated_data_dir();
    crate::test_env::remove_var("LEAN_CTX_PROJECT_ROOT");

    let pairs = mcp_server_env_pairs();
    let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
    assert!(!keys.contains(&"LEAN_CTX_DATA_DIR"));
    assert!(!keys.contains(&"LEAN_CTX_PROJECT_ROOT"));
    assert!(!keys.contains(&"LEAN_CTX_EXTRA_ROOTS"));
}

// ── #708: portable hook-binary override for multi-machine synced configs ──

#[test]
fn hook_command_binary_honors_override_and_skips_msys_rewrite() {
    let iso = crate::core::data_dir::isolated_data_dir();
    let cfg = iso.path().join("config.toml");
    std::fs::write(&cfg, "hook_binary = \"$HOME/.local/bin/lean-ctx\"\n").unwrap();
    assert_eq!(resolve_hook_command_binary(), "$HOME/.local/bin/lean-ctx");
    // The override is already the user's chosen shell form — never rewritten
    // into the MSYS `/c/…` form.
    assert_eq!(resolve_binary_path_for_bash(), "$HOME/.local/bin/lean-ctx");

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
            "$USERPROFILE/AppData/Roaming/npm/node_modules/lean-ctx-bin/bin/lean-ctx.exe hook rewrite"
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

// ── #719: wrapper scripts must honor the override and survive healing ──

#[test]
fn claude_wrapper_scripts_emit_override_verbatim_and_quoted() {
    let iso = crate::core::data_dir::isolated_data_dir();
    // A portable form WITH a space — quoting is part of the contract.
    std::fs::write(
        iso.path().join("config.toml"),
        "hook_binary = \"$HOME/App Data/npm/node_modules/lean-ctx-bin/bin/lean-ctx.exe\"\n",
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();

    install_claude_hook_scripts(home.path());

    let hooks_dir = home.path().join(".claude/hooks");
    for (file, needle) in [
        (
            "lean-ctx-rewrite-native",
            "exec \"$HOME/App Data/npm/node_modules/lean-ctx-bin/bin/lean-ctx.exe\" hook rewrite",
        ),
        (
            "lean-ctx-redirect-native",
            "exec \"$HOME/App Data/npm/node_modules/lean-ctx-bin/bin/lean-ctx.exe\" hook redirect",
        ),
        (
            "lean-ctx-rewrite.sh",
            "LEAN_CTX_BIN=\"$HOME/App Data/npm/node_modules/lean-ctx-bin/bin/lean-ctx.exe\"",
        ),
    ] {
        let content = std::fs::read_to_string(hooks_dir.join(file)).expect(file);
        assert!(
            content.contains(needle),
            "{file} must carry the quoted portable form, got:\n{content}"
        );
        assert!(
            !content.contains(&resolve_binary_path()),
            "{file}: no machine-absolute path may leak into a synced wrapper"
        );
    }
}

#[test]
fn heal_without_override_preserves_working_portable_wrapper() {
    let _iso = crate::core::data_dir::isolated_data_dir();
    let home = tempfile::tempdir().unwrap();

    // The synced peer's portable wrapper resolves on THIS machine: the
    // binary it points at exists under this home.
    let bin_dir = home.path().join(".local/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(bin_dir.join("lean-ctx"), "#!/bin/sh\nexit 0\n").unwrap();

    let hooks_dir = home.path().join(".claude/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let portable = "#!/bin/sh\nexec \"$HOME/.local/bin/lean-ctx\" hook rewrite\n";
    let wrapper = hooks_dir.join("lean-ctx-rewrite-native");
    std::fs::write(&wrapper, portable).unwrap();

    // Session heal on the machine WITHOUT the override (the #719 scenario):
    // the working portable wrapper must survive byte-for-byte.
    install_claude_hook_scripts(home.path());
    assert_eq!(
        std::fs::read_to_string(&wrapper).unwrap(),
        portable,
        "heal must not stamp a machine-absolute path over a working portable wrapper"
    );

    // A portable wrapper whose binary does NOT resolve here is genuinely
    // broken — healing must replace it.
    let broken = "#!/bin/sh\nexec \"$HOME/nonexistent/lean-ctx\" hook redirect\n";
    let redirect = hooks_dir.join("lean-ctx-redirect-native");
    std::fs::write(&redirect, broken).unwrap();
    install_claude_hook_scripts(home.path());
    let healed = std::fs::read_to_string(&redirect).unwrap();
    assert_ne!(healed, broken, "a dead portable wrapper must be healed");
    assert!(healed.contains(" hook redirect"));
}

#[test]
fn wrapper_binary_token_parses_all_generated_forms() {
    // Quoted native wrapper.
    assert_eq!(
        wrapper_binary_token("#!/bin/sh\nexec \"$HOME/b in/lean-ctx\" hook rewrite\n").as_deref(),
        Some("$HOME/b in/lean-ctx")
    );
    // Rewrite script assignment.
    assert_eq!(
        wrapper_binary_token("set -euo pipefail\nLEAN_CTX_BIN=\"$HOME/x/lean-ctx\"\n").as_deref(),
        Some("$HOME/x/lean-ctx")
    );
    assert_eq!(wrapper_binary_token("#!/bin/sh\nexit 0\n"), None);
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
    std::fs::write(&hooks, "{\"hooks\":{\"cmd\":\"lean-ctx hook rewrite\"}}").unwrap();
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

#[test]
fn rewrite_script_uses_registry_pattern() {
    let script = generate_rewrite_script("/usr/bin/lean-ctx");
    assert!(script.contains(r"git\ *"), "script missing git pattern");
    assert!(script.contains(r"cargo\ *"), "script missing cargo pattern");
    assert!(script.contains(r"npm\ *"), "script missing npm pattern");
    assert!(script.contains(r"rg\ *"), "script missing rg pattern");
    assert!(script.contains(r"ls\ *"), "script missing ls pattern");
    assert!(
        script.contains("LEAN_CTX_BIN=\"/usr/bin/lean-ctx\""),
        "script missing binary path"
    );
    assert!(
        script.contains("PowerShell|powershell"),
        "rewrite script must accept PowerShell tool names for Windows compatibility"
    );
}

#[test]
fn rewrite_scripts_contain_all_registry_commands() {
    let script = generate_rewrite_script("lean-ctx");
    let compact = generate_compact_rewrite_script("lean-ctx");
    for entry in crate::rewrite_registry::REWRITE_COMMANDS {
        if matches!(entry.category, crate::rewrite_registry::Category::FileRead) {
            continue;
        }
        let pattern = if entry.command.contains('-') {
            format!("{}*", entry.command.replace('-', r"\-"))
        } else {
            format!(r"{}\ *", entry.command)
        };
        assert!(
            script.contains(&pattern),
            "rewrite_script missing '{}' (pattern: {})",
            entry.command,
            pattern
        );
        assert!(
            compact.contains(&pattern),
            "compact_rewrite_script missing '{}' (pattern: {})",
            entry.command,
            pattern
        );
    }
}

#[test]
fn rewrite_script_skips_multiline_commands() {
    let script = generate_rewrite_script("lean-ctx");
    assert!(
        script.contains(r"grep -qF '\n'"),
        "rewrite script must guard against unresolved JSON \\n (#787)"
    );
    let compact = generate_compact_rewrite_script("lean-ctx");
    assert!(
        compact.contains(r"grep -qF '\n'"),
        "compact rewrite script must guard against unresolved JSON \\n (#787)"
    );
}

#[test]
fn codex_is_replace() {
    let _env = crate::core::data_dir::test_env_lock();
    assert_eq!(recommend_hook_mode("codex"), HookMode::Replace);
}

#[test]
fn cursor_is_replace() {
    let _env = crate::core::data_dir::test_env_lock();
    assert_eq!(recommend_hook_mode("cursor"), HookMode::Replace);
}

#[test]
fn gemini_is_replace() {
    let _env = crate::core::data_dir::test_env_lock();
    assert_eq!(recommend_hook_mode("gemini"), HookMode::Replace);
}

#[test]
fn claude_is_replace() {
    let _env = crate::core::data_dir::test_env_lock();
    assert_eq!(recommend_hook_mode("claude"), HookMode::Replace);
}

#[test]
fn hybrid_fallback_agents() {
    let _env = crate::core::data_dir::test_env_lock();
    assert_eq!(recommend_hook_mode("copilot"), HookMode::Hybrid);
}

#[test]
fn unknown_agent_falls_back_to_mcp() {
    let _env = crate::core::data_dir::test_env_lock();
    assert_eq!(recommend_hook_mode("unknown-agent"), HookMode::Mcp);
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
