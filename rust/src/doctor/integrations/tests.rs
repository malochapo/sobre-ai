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
        note.detail.contains("sobre_shell") && note.detail.contains("every surface"),
        "note must steer users to the MCP tools as the reliable cross-surface path: {}",
        note.detail
    );
}

#[test]
fn hook_binary_refs_extracts_token_before_hook_keyword() {
    let content =
        r#"{"command": "/opt/sobre hook compress"} {"command": "/opt/sobre hook observe"}"#;
    let refs = hook_binary_refs(content);
    assert_eq!(refs, vec!["/opt/sobre", "/opt/sobre"]);
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
        r#"[{"hooks":[{"command":"sobre hook compress"},{"command":"sobre hook observe"}]}]"#;
    assert_eq!(hook_binary_refs(content), vec!["sobre", "sobre"]);
}

#[test]
fn stale_hook_binary_accepts_minified_bare_command() {
    let content = r#"[{"hooks":[{"command":"sobre hook compress"}]}]"#;
    assert!(stale_hook_binary(content, "/anything/sobre").is_none());
}

#[test]
fn stale_hook_binary_flags_minified_foreign_path() {
    let content = r#"[{"hooks":[{"command":"/old/install/sobre hook compress"}]}]"#;
    assert_eq!(
        stale_hook_binary(content, "/current/sobre").as_deref(),
        Some("/old/install/sobre")
    );
}

#[test]
fn stale_hook_binary_flags_foreign_path() {
    let content = r#""/nonexistent/old/sobre hook compress""#;
    let stale = stale_hook_binary(content, "/current/install/sobre");
    assert_eq!(stale.as_deref(), Some("/nonexistent/old/sobre"));
}

#[test]
fn stale_hook_binary_accepts_current_binary() {
    let bin = "/current/install/sobre";
    let content = format!(r#""{bin} hook compress""#);
    assert!(stale_hook_binary(&content, bin).is_none());
}

#[test]
fn stale_hook_binary_accepts_bare_path_command() {
    // The bare `sobre` PATH form is always considered current.
    let content = r#""sobre hook compress""#;
    assert!(stale_hook_binary(content, "/anything/sobre").is_none());
}

/// #708: a configured verbatim hook binary ($HOME/... synced across machines)
/// is the expected command — doctor must not flag it stale, or --fix would
/// rewrite it back to an absolute path and restart the sync ping-pong.
#[test]
fn stale_hook_binary_accepts_configured_portable_override() {
    let iso = crate::core::data_dir::isolated_data_dir();
    let cfg = iso.path().join("config.toml");
    std::fs::write(&cfg, "hook_binary = \"$HOME/.local/bin/sobre\"\n").unwrap();
    let content = r#""$HOME/.local/bin/sobre hook compress""#;
    assert!(stale_hook_binary(content, "/machine/abs/sobre").is_none());
    std::fs::remove_file(&cfg).unwrap();

    // Without the override the same content IS stale — the acceptance is
    // strictly opt-in.
    assert_eq!(
        stale_hook_binary(content, "/machine/abs/sobre").as_deref(),
        Some("$HOME/.local/bin/sobre")
    );
}

#[test]
fn finalize_hook_check_reports_drift_missing_and_stale() {
    let p = std::path::Path::new("/tmp/hooks.json");

    let missing = finalize_hook_check("Hooks", p, false, None);
    assert!(!missing.ok);
    assert!(missing.detail.contains("drift"));

    let stale = finalize_hook_check("Hooks", p, true, Some("/old/sobre".to_string()));
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
  { "matcher": "Shell", "command": "/old/bin/sobre hook cursor" }
]
  }
}"#,
    )
    .unwrap();
    let check = check_cursor_hooks(&path, "/new/bin/sobre");
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
  { "matcher": "Shell", "command": "sobre hook cursor" }
]
  }
}"#,
    )
    .unwrap();
    let check = check_cursor_hooks(&path, "/new/bin/sobre");
    assert!(check.ok, "bare sobre command is PATH-resolved and current");
}
