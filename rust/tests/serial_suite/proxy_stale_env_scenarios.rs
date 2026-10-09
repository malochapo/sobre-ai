//! Scenario tests:
//! Stale `ANTHROPIC_BASE_URL` detection and cleanup when proxy is not enabled.
//!
//! Each test uses an isolated temp dir for both `SOBRE_DATA_DIR` (data dir)
//! and `CLAUDE_CONFIG_DIR` (Claude Code settings) to avoid interference
//! with the host system's real dirs.

struct TestEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let data_dir = home.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        unsafe { std::env::set_var("SOBRE_DATA_DIR", &data_dir) };
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude")) };
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        unsafe { std::env::set_var("CODEX_HOME", home.join(".codex")) };
        Self { _tmp: tmp, home }
    }

    fn set_claude_settings(&self, json: &str) {
        let claude_dir = self.home.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("settings.json"), json).unwrap();
    }

    fn read_claude_settings(&self) -> serde_json::Value {
        let path = self.home.join(".claude/settings.json");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        unsafe { std::env::remove_var("SOBRE_DATA_DIR") };
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        unsafe { std::env::remove_var("CODEX_HOME") };
    }
}

// ---------------------------------------------------------------------------
// Scenario 1: is_local_sobre_url correctly identifies local proxy URLs
// ---------------------------------------------------------------------------

#[test]
fn scenario_local_url_detection() {
    use sobre::proxy_setup::is_local_sobre_url;

    assert!(is_local_sobre_url("http://127.0.0.1:4444"));
    assert!(is_local_sobre_url("http://localhost:4444"));
    assert!(is_local_sobre_url("http://127.0.0.1:5555"));
    assert!(is_local_sobre_url("http://localhost:3333"));

    assert!(!is_local_sobre_url("https://api.anthropic.com"));
    assert!(!is_local_sobre_url("https://proxy.company.com:4444"));
    assert!(!is_local_sobre_url(""));
    assert!(!is_local_sobre_url("http://192.168.1.1:4444"));
}

// ---------------------------------------------------------------------------
// Scenario 2: cleanup_stale_proxy_env removes local URL when proxy disabled
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_removes_stale_url() {
    let env = TestEnv::new();
    env.set_claude_settings(r#"{"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:4444"}}"#);

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert!(cleaned > 0, "should have cleaned stale URL");

    let doc = env.read_claude_settings();
    let has_url = doc
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .is_some();
    assert!(!has_url, "ANTHROPIC_BASE_URL should be removed");
}

// ---------------------------------------------------------------------------
// Scenario 3: cleanup does nothing when no stale URL exists
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_noop_when_no_stale_url() {
    let env = TestEnv::new();
    env.set_claude_settings(r#"{"env": {"SOME_OTHER_VAR": "value"}}"#);

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert_eq!(cleaned, 0, "nothing to clean");
}

// ---------------------------------------------------------------------------
// Scenario 4: cleanup preserves non-local ANTHROPIC_BASE_URL
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_preserves_remote_url() {
    let env = TestEnv::new();
    env.set_claude_settings(r#"{"env": {"ANTHROPIC_BASE_URL": "https://api.anthropic.com"}}"#);

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert_eq!(cleaned, 0, "should not touch remote URL");

    let doc = env.read_claude_settings();
    let url = doc
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        url, "https://api.anthropic.com",
        "remote URL must be preserved"
    );
}

// ---------------------------------------------------------------------------
// Scenario 5: cleanup handles missing settings.json gracefully
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_missing_settings_file() {
    let env = TestEnv::new();
    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert_eq!(cleaned, 0, "should handle missing file gracefully");
}

// ---------------------------------------------------------------------------
// Scenario 6: cleanup handles malformed JSON gracefully
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_malformed_json() {
    let env = TestEnv::new();
    env.set_claude_settings("this is not json {{{");

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert_eq!(cleaned, 0, "should handle malformed JSON gracefully");
}

// ---------------------------------------------------------------------------
// Scenario 7: cleanup handles empty env object
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_empty_env_object() {
    let env = TestEnv::new();
    env.set_claude_settings(r#"{"env": {}}"#);

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert_eq!(cleaned, 0, "nothing to clean with empty env");
}

// ---------------------------------------------------------------------------
// Scenario 8: cleanup with localhost variant
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_localhost_variant() {
    let env = TestEnv::new();
    env.set_claude_settings(r#"{"env": {"ANTHROPIC_BASE_URL": "http://localhost:4444"}}"#);

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert!(cleaned > 0, "should clean localhost variant too");
}

// ---------------------------------------------------------------------------
// Scenario 9: cleanup preserves other env vars and settings
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_preserves_other_env_vars() {
    let env = TestEnv::new();
    env.set_claude_settings(
        r#"{"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:4444", "OTHER_VAR": "keep_me"}, "hooks": {"PreToolUse": []}}"#,
    );

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert!(cleaned > 0);

    let doc = env.read_claude_settings();
    let other = doc
        .get("env")
        .and_then(|e| e.get("OTHER_VAR"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(other, "keep_me", "other env vars must be preserved");

    assert!(doc.get("hooks").is_some(), "hooks must be preserved");
}

// ---------------------------------------------------------------------------
// Scenario 10: stale_proxy_agents names every agent left on the proxy
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_has_stale_url_detection() {
    use sobre::proxy_setup::stale_proxy_agents;

    let env = TestEnv::new();

    assert!(
        stale_proxy_agents(&env.home).is_empty(),
        "no file = no stale URL"
    );

    env.set_claude_settings(r#"{"env": {"ANTHROPIC_BASE_URL": "https://api.anthropic.com"}}"#);
    assert!(
        stale_proxy_agents(&env.home).is_empty(),
        "remote URL is not stale"
    );

    env.set_claude_settings(r#"{"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:4444"}}"#);
    assert_eq!(
        stale_proxy_agents(&env.home),
        ["Claude Code"],
        "local URL with proxy disabled is stale"
    );

    let opencode = env.home.join(".config/opencode");
    std::fs::create_dir_all(&opencode).unwrap();
    std::fs::write(
        opencode.join("opencode.json"),
        r#"{"provider": {"openai": {"options": {"baseURL": "http://127.0.0.1:5555/v1"}}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(env.home.join(".codex")).unwrap();
    std::fs::write(
        env.home.join(".codex/config.toml"),
        "openai_base_url = \"http://127.0.0.1:4444/v1\"\n",
    )
    .unwrap();
    assert_eq!(
        stale_proxy_agents(&env.home),
        ["Claude Code", "Codex", "OpenCode"]
    );
}

// ---------------------------------------------------------------------------
// Scenario 11: cleanup with non-default port
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_cleanup_non_default_port() {
    let env = TestEnv::new();
    env.set_claude_settings(r#"{"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:5555"}}"#);

    let cleaned = sobre::proxy_setup::cleanup_stale_proxy_env(&env.home);
    assert!(cleaned > 0, "should clean non-default port too");
}

// ---------------------------------------------------------------------------
// Scenario 12: install_proxy_env guard prevents writing when proxy not enabled
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn scenario_install_guard_prevents_writing_when_disabled() {
    let env = TestEnv::new();
    env.set_claude_settings(r"{}");

    sobre::proxy_setup::install_proxy_env(&env.home, 4444, true);

    let doc = env.read_claude_settings();
    let has_url = doc
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .is_some();
    assert!(
        !has_url,
        "install_proxy_env should not write URL when proxy is not enabled"
    );
}
