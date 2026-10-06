#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn codex_desktop_note_is_informational_and_never_fails() {
    let note = codex_desktop_note();
    assert!(
        note.ok,
        "the Codex Desktop note is informational, never a failure"
    );
    assert!(
        note.detail.contains("ctx_shell") && note.detail.contains("every surface"),
        "note must steer users to the MCP tools as the reliable cross-surface path: {}",
        note.detail
    );
}

#[test]
fn claude_flags_instructions_advertising_ctx_without_mcp_registration() {
    // GH #637 (second half) / GL #1139: a CLAUDE.md block advertising ctx_*
    // tools while no lean-ctx MCP server is registered strands the agent on
    // fallbacks that do not exist in the session. The combination must be
    // surfaced as its own failing check with a repair hint.
    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    // Hermetic config: the developer's real config must not leak in.
    crate::test_env::set_var(
        "LEAN_CTX_CONFIG_DIR",
        home.join("cfg").to_string_lossy().to_string(),
    );
    let claude_dir = crate::core::editor_registry::claude_state_dir(home);
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("CLAUDE.md"),
        format!(
            "{}\n## lean-ctx\nctx_read guidance…\n{}\n",
            crate::hooks::agents::CLAUDE_MD_BLOCK_START,
            crate::core::rules_canonical::AGENTS_BLOCK_END
        ),
    )
    .unwrap();
    // No ~/.claude.json → MCP registration missing.

    let status = integration_claude(home, "/usr/local/bin/lean-ctx");
    let consistency = status
        .checks
        .iter()
        .find(|c| c.name == "Instructions/MCP consistency");
    let consistency =
        consistency.expect("block-without-MCP must produce the Instructions/MCP consistency check");
    assert!(!consistency.ok, "the combination is a failure, not a note");
    assert!(
        consistency.detail.contains("lean-ctx init"),
        "detail must carry the repair hint: {}",
        consistency.detail
    );

    // With the MCP server registered the consistency check disappears.
    std::fs::write(
        crate::core::editor_registry::claude_mcp_json_path(home),
        r#"{"mcpServers":{"lean-ctx":{"command":"/usr/local/bin/lean-ctx","args":[]}}}"#,
    )
    .unwrap();
    let healthy = integration_claude(home, "/usr/local/bin/lean-ctx");
    assert!(
        !healthy
            .checks
            .iter()
            .any(|c| c.name == "Instructions/MCP consistency"),
        "registered MCP must suppress the consistency warning"
    );
    crate::test_env::remove_var("LEAN_CTX_CONFIG_DIR");
}

#[test]
fn hook_binary_refs_extracts_token_before_hook_keyword() {
    let content =
        r#"{"command": "/opt/lean-ctx hook rewrite"} {"command": "/opt/lean-ctx hook redirect"}"#;
    let refs = hook_binary_refs(content);
    assert_eq!(refs, vec!["/opt/lean-ctx", "/opt/lean-ctx"]);
}

#[test]
fn hook_binary_refs_empty_without_hook_invocation() {
    assert!(hook_binary_refs(r#"{"command": "echo nothing here"}"#).is_empty());
}

#[test]
fn hook_binary_refs_handles_minified_json() {
    // `serde_json::to_string` emits no spaces around keys/values; the binary
    // token must still be extracted cleanly. Regression: the whitespace-only
    // split used to capture the entire JSON prefix as the "binary".
    let content =
        r#"[{"hooks":[{"command":"lean-ctx hook rewrite"},{"command":"lean-ctx hook redirect"}]}]"#;
    assert_eq!(hook_binary_refs(content), vec!["lean-ctx", "lean-ctx"]);
}

#[test]
fn stale_hook_binary_accepts_minified_bare_command() {
    let content = r#"[{"hooks":[{"command":"lean-ctx hook rewrite"}]}]"#;
    assert!(stale_hook_binary(content, "/anything/lean-ctx").is_none());
}

#[test]
fn stale_hook_binary_flags_minified_foreign_path() {
    let content = r#"[{"hooks":[{"command":"/old/install/lean-ctx hook rewrite"}]}]"#;
    assert_eq!(
        stale_hook_binary(content, "/current/lean-ctx").as_deref(),
        Some("/old/install/lean-ctx")
    );
}

#[test]
fn stale_hook_binary_flags_foreign_path() {
    let content = r#""/nonexistent/old/lean-ctx hook rewrite""#;
    let stale = stale_hook_binary(content, "/current/install/lean-ctx");
    assert_eq!(stale.as_deref(), Some("/nonexistent/old/lean-ctx"));
}

#[test]
fn stale_hook_binary_accepts_current_binary() {
    let bin = "/current/install/lean-ctx";
    let content = format!(r#""{bin} hook rewrite""#);
    assert!(stale_hook_binary(&content, bin).is_none());
}

#[test]
fn stale_hook_binary_accepts_bare_path_command() {
    // The bare `lean-ctx` PATH form is always considered current.
    let content = r#""lean-ctx hook rewrite""#;
    assert!(stale_hook_binary(content, "/anything/lean-ctx").is_none());
}

/// #708: a configured verbatim hook binary ($HOME/... synced across machines)
/// is the expected command — doctor must not flag it stale, or --fix would
/// rewrite it back to an absolute path and restart the sync ping-pong.
#[test]
fn stale_hook_binary_accepts_configured_portable_override() {
    let iso = crate::core::data_dir::isolated_data_dir();
    let cfg = iso.path().join("config.toml");
    std::fs::write(&cfg, "hook_binary = \"$HOME/.local/bin/lean-ctx\"\n").unwrap();
    let content = r#""$HOME/.local/bin/lean-ctx hook rewrite""#;
    assert!(stale_hook_binary(content, "/machine/abs/lean-ctx").is_none());
    std::fs::remove_file(&cfg).unwrap();

    // Without the override the same content IS stale — the acceptance is
    // strictly opt-in.
    assert_eq!(
        stale_hook_binary(content, "/machine/abs/lean-ctx").as_deref(),
        Some("$HOME/.local/bin/lean-ctx")
    );
}

#[test]
fn finalize_hook_check_reports_drift_missing_and_stale() {
    let p = std::path::Path::new("/tmp/hooks.json");

    let missing = finalize_hook_check("Hooks", p, false, None);
    assert!(!missing.ok);
    assert!(missing.detail.contains("drift"));

    let stale = finalize_hook_check("Hooks", p, true, Some("/old/lean-ctx".to_string()));
    assert!(!stale.ok);
    assert!(stale.detail.contains("stale binary"));
    assert!(stale.detail.contains("init --repair"));

    let healthy = finalize_hook_check("Hooks", p, true, None);
    assert!(healthy.ok);
    assert!(healthy.detail.contains("ok"));
}

#[test]
fn check_cursor_hooks_detects_stale_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("hooks.json");
    std::fs::write(
        &path,
        r#"{
  "hooks": {
"preToolUse": [
  { "matcher": "Shell", "command": "/old/bin/lean-ctx hook rewrite" },
  { "matcher": "Read|Grep", "command": "/old/bin/lean-ctx hook redirect" }
]
  }
}"#,
    )
    .unwrap();
    let check = check_cursor_hooks(&path, "/new/bin/lean-ctx");
    assert!(!check.ok, "stale binary path must fail the hook check");
    assert!(check.detail.contains("stale binary"));
}

#[test]
fn check_cursor_hooks_ok_for_bare_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("hooks.json");
    std::fs::write(
        &path,
        r#"{
  "hooks": {
"preToolUse": [
  { "matcher": "Shell", "command": "lean-ctx hook rewrite" },
  { "matcher": "Read|Grep", "command": "lean-ctx hook redirect" }
]
  }
}"#,
    )
    .unwrap();
    let check = check_cursor_hooks(&path, "/new/bin/lean-ctx");
    assert!(
        check.ok,
        "bare lean-ctx command is PATH-resolved and current"
    );
}
