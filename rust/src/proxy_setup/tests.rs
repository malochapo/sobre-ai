use super::claude::*;
use super::codex::*;
use super::util::*;
use super::*;

#[test]
fn uid_port_first_regular_user() {
    // uid 1000 (first regular user on most Linux) → base port
    assert_eq!(DEFAULT_PROXY_PORT, 4444);
}

#[test]
fn proxy_timeout_default_200ms() {
    // Private config without `proxy_timeout_ms` → the built-in default.
    let _iso = crate::core::data_dir::isolated_data_dir();
    assert_eq!(proxy_timeout(), std::time::Duration::from_millis(200));
}

#[test]
fn is_proxy_reachable_returns_false_on_unused_port() {
    assert!(!is_proxy_reachable(19999));
}

/// The subscription guard reads the process environment; these tests are only
/// meaningful when the test runner itself does not provide an Anthropic key.
fn env_provides_anthropic_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok_and(|v| !v.trim().is_empty())
        || std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok_and(|v| !v.trim().is_empty())
}

/// `claude_state_dir` honours `CLAUDE_CONFIG_DIR`; when set it would escape the
/// temp HOME and read the real settings file, so skip in that case.
fn claude_dir_overridden() -> bool {
    std::env::var("CLAUDE_CONFIG_DIR").is_ok_and(|v| !v.trim().is_empty())
}

fn write_claude_settings(home: &Path, json: &str) -> std::path::PathBuf {
    let dir = home.join(".claude");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, json).unwrap();
    path
}

#[test]
fn api_key_available_true_with_api_key_helper() {
    if claude_dir_overridden() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    write_claude_settings(home.path(), r#"{"apiKeyHelper": "echo sk-test"}"#);
    assert!(anthropic_api_key_available(home.path()));
}

#[test]
fn api_key_available_true_with_settings_env_key() {
    if claude_dir_overridden() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    write_claude_settings(home.path(), r#"{"env": {"ANTHROPIC_API_KEY": "sk-test"}}"#);
    assert!(anthropic_api_key_available(home.path()));
}

#[test]
fn api_key_available_false_without_key() {
    if env_provides_anthropic_key() || claude_dir_overridden() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    write_claude_settings(home.path(), r#"{"env": {}}"#);
    assert!(!anthropic_api_key_available(home.path()));
}

#[test]
fn api_key_available_false_when_no_settings_file() {
    if env_provides_anthropic_key() || claude_dir_overridden() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    assert!(!anthropic_api_key_available(home.path()));
}

#[test]
fn subscription_guard_skips_redirect_without_key() {
    if env_provides_anthropic_key() || claude_dir_overridden() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    // No settings file → subscription mode, empty current URL → nothing to repair.
    install_claude_env_inner(home.path(), 4444, true, false);
    let settings = home.path().join(".claude/settings.json");
    assert!(
        !settings.exists(),
        "subscription mode must not write a proxy redirect"
    );
}

#[test]
fn subscription_guard_repairs_stale_local_redirect() {
    if env_provides_anthropic_key() || claude_dir_overridden() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let path = write_claude_settings(
        home.path(),
        r#"{"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:4444"}}"#,
    );
    install_claude_env_inner(home.path(), 4444, true, false);
    let after = std::fs::read_to_string(&path).unwrap();
    let doc: serde_json::Value = crate::core::jsonc::parse_jsonc(&after).unwrap();
    let base = doc
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        !is_local_lean_ctx_url(base),
        "stale local redirect must be repaired in subscription mode, got {base:?}"
    );
}

/// API-key mode must STILL route Claude through the proxy (we only protect
/// subscriptions; pay-as-you-go users keep their compression). Uses a real bound
/// port so `is_proxy_reachable` passes, exercising the full production path.
#[test]
fn install_redirects_claude_when_api_key_present() {
    if claude_dir_overridden() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    // API-key mode declared in settings.json → deterministic regardless of env.
    write_claude_settings(home.path(), r#"{"env": {"ANTHROPIC_API_KEY": "sk-test"}}"#);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    install_claude_env_inner(home.path(), port, true, false);

    let after = std::fs::read_to_string(home.path().join(".claude/settings.json")).unwrap();
    let doc: serde_json::Value = crate::core::jsonc::parse_jsonc(&after).unwrap();
    let base = doc
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        base,
        format!("http://127.0.0.1:{port}"),
        "API-key mode must route Claude through the proxy"
    );
}

/// Codex CLI config: a fresh install writes the `/v1`-suffixed proxy URL (#366).
#[test]
fn codex_env_writes_v1_suffixed_url() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    install_codex_env_at(&codex_dir, port, true);

    let cfg = std::fs::read_to_string(codex_dir.join("config.toml")).unwrap();
    assert!(
        cfg.contains(&format!("openai_base_url = \"http://127.0.0.1:{port}/v1\"")),
        "Codex config must set top-level openai_base_url with the /v1 suffix, got:\n{cfg}"
    );
    assert!(
        !cfg.contains("[env]") && !cfg.contains("OPENAI_BASE_URL"),
        "must not write the dead [env] OPENAI_BASE_URL form (#554), got:\n{cfg}"
    );
    assert!(
        !cfg.contains("chatgpt_base_url"),
        "API-key mode must not install the ChatGPT backend rail, got:\n{cfg}"
    );
}

/// Codex CLI config: a custom non-local `openai_base_url` is never rewritten.
#[test]
fn codex_env_preserves_custom_remote_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let original = "openai_base_url = \"https://my-gateway.example.com/v1\"\n";
    std::fs::write(codex_dir.join("config.toml"), original).unwrap();

    install_codex_env_at(&codex_dir, port, true);

    let cfg = std::fs::read_to_string(codex_dir.join("config.toml")).unwrap();
    assert!(
        cfg.contains("https://my-gateway.example.com/v1"),
        "custom remote endpoint must be preserved, got:\n{cfg}"
    );
    assert!(
        !cfg.contains("127.0.0.1"),
        "proxy URL must not be injected over a custom endpoint"
    );
}

/// A ChatGPT login leaves Codex native — no `model_provider` pin (which would
/// scope/hide history), no `chatgpt_base_url`, no provider block, no proxy URL.
#[test]
fn codex_env_chatgpt_mode_optout_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(codex_dir.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();

    install_codex_env_at_mode(&codex_dir, port, true, CodexProxyMode::ChatGpt);

    let cfg = std::fs::read_to_string(codex_dir.join("config.toml")).unwrap();
    assert!(
        !cfg.contains("model_provider"),
        "ChatGPT mode must not pin a model_provider (#597), got:\n{cfg}"
    );
    assert!(
        !cfg.contains("chatgpt_base_url") && !cfg.contains("openai_base_url"),
        "ChatGPT mode must not write any proxy base URL, got:\n{cfg}"
    );
    assert!(
        !cfg.contains("127.0.0.1"),
        "ChatGPT mode must not install the provider block or any proxy URL, got:\n{cfg}"
    );
    assert!(cfg.contains("model = \"gpt-5.5\""), "user keys preserved");
}

/// `render_codex_config` is idempotent: applying it to an already-configured
/// body yields the identical body (so `install` reports "already configured").
#[test]
fn render_codex_config_is_idempotent() {
    let entries = vec![("openai_base_url", "http://127.0.0.1:4444/v1".to_string())];
    let once = render_codex_config("model = \"gpt-5.5\"\n", &entries);
    let twice = render_codex_config(&once, &entries);
    assert_eq!(once, twice, "render must be idempotent");
    assert!(once.starts_with("openai_base_url = \"http://127.0.0.1:4444/v1\"\n"));
    assert!(once.contains("model = \"gpt-5.5\""));
}

/// `render_codex_config` inserts the key as a *top-level* key (before the first
/// `[table]`), otherwise Codex would read it as a sub-key and ignore it.
#[test]
fn render_codex_config_inserts_before_first_table() {
    let body = "model = \"gpt-5.5\"\n\n[features]\nhooks = true\n";
    let entries = vec![("openai_base_url", "http://127.0.0.1:4444/v1".to_string())];
    let out = render_codex_config(body, &entries);
    let key_idx = out.find("openai_base_url").expect("key present");
    let table_idx = out.find("[features]").expect("table present");
    assert!(
        key_idx < table_idx,
        "openai_base_url must precede the first table, got:\n{out}"
    );
}

/// `auth_is_chatgpt` reflects Codex's `auth.json` auth mode.
#[test]
fn auth_is_chatgpt_detects_login_mode() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();

    assert!(!auth_is_chatgpt(&codex_dir), "no auth.json => not chatgpt");

    std::fs::write(
        codex_dir.join("auth.json"),
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-test"}"#,
    )
    .unwrap();
    assert!(!auth_is_chatgpt(&codex_dir), "apikey mode => not chatgpt");

    std::fs::write(
        codex_dir.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"x"}}"#,
    )
    .unwrap();
    assert!(auth_is_chatgpt(&codex_dir), "chatgpt mode => true");

    for mode in ["chatgptAuthTokens", "personalAccessToken", "agentIdentity"] {
        std::fs::write(
            codex_dir.join("auth.json"),
            format!(r#"{{"auth_mode":"{mode}","tokens":{{"access_token":"x"}}}}"#),
        )
        .unwrap();
        assert!(auth_is_chatgpt(&codex_dir), "{mode} => true");
    }
}
