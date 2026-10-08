//! End-to-end regression: `proxy enable` must treat the two
//! Codex auth modes differently.
//!
//! - **API-key** mode: Codex is pointed at the proxy's `/v1` rail (top-level
//!   `openai_base_url`).
//! - **ChatGPT login**: left native — no sobre Codex config — so history and
//!   `codex cloud`/remote keep working.
//!
//! All scenarios live in one serial test: they redirect Codex via `CODEX_HOME`
//! (the documented override `resolve_codex_dir` honours) and a live dummy proxy so
//! the reachability guard passes. A single test means no in-process race on the
//! shared env vars, and a dedicated test binary isolates it from the lib suite.

use std::ffi::OsString;
use std::net::TcpListener;
use std::path::Path;

/// The four XDG category overrides that the crate-private `data_dir::isolated_data_dir()`
/// sets to isolate a test's config/data/state/cache. Re-declared here because that
/// helper is `#[cfg(test)]` and therefore unavailable to this integration-test binary.
const ISOLATED_ENV_VARS: [&str; 4] = [
    "SOBRE_DATA_DIR",
    "SOBRE_CONFIG_DIR",
    "SOBRE_STATE_DIR",
    "SOBRE_CACHE_DIR",
];

/// Scope-guard that points `CODEX_HOME` at `dir` and restores the previous value on
/// drop. `set_var`/`remove_var` are `unsafe` on edition 2024; safe here because this
/// test binary runs the single test below serially.
struct CodexHome(Option<OsString>);

impl CodexHome {
    fn set(dir: &Path) -> Self {
        let prev = std::env::var_os("CODEX_HOME");
        // SAFETY: the project's test suite always runs with `--test-threads=1`
        // (env-race legacy — see .github/workflows/ci.yml), so no other test in
        // this binary touches the environment concurrently.
        unsafe { std::env::set_var("CODEX_HOME", dir) };
        CodexHome(prev)
    }
}

impl Drop for CodexHome {
    fn drop(&mut self) {
        match &self.0 {
            // SAFETY: the project's test suite always runs with `--test-threads=1`
            // (env-race legacy — see .github/workflows/ci.yml), so no other test in
            // this binary touches the environment concurrently.
            Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
            // SAFETY: the project's test suite always runs with `--test-threads=1`
            // (env-race legacy — see .github/workflows/ci.yml), so no other test in
            // this binary touches the environment concurrently.
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }
    }
}

/// Scope-guard for an arbitrary env var that restores the previous value on drop.
/// Same `unsafe`/serial-test caveat as [`CodexHome`].
struct EnvVar {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: the project's test suite always runs with `--test-threads=1`
        // (env-race legacy — see .github/workflows/ci.yml), so no other test in
        // this binary touches the environment concurrently.
        unsafe { std::env::set_var(key, value) };
        EnvVar { key, prev }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: the project's test suite always runs with `--test-threads=1`
            // (env-race legacy — see .github/workflows/ci.yml), so no other test in
            // this binary touches the environment concurrently.
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            // SAFETY: the project's test suite always runs with `--test-threads=1`
            // (env-race legacy — see .github/workflows/ci.yml), so no other test in
            // this binary touches the environment concurrently.
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn dummy_proxy_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Writes a Codex `.codex` dir with the given auth mode and an unrelated config key,
/// then returns the temp dir (kept alive by the caller) and the `.codex` path.
fn codex_home_with_auth(auth_json: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let codex = home.path().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(codex.join("auth.json"), auth_json).unwrap();
    std::fs::write(codex.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();
    (home, codex)
}

#[test]
fn proxy_enable_respects_codex_auth_mode_554() {
    // An explicit CODEX_API_KEY forces API-key mode regardless of auth.json, which
    // would invalidate the ChatGPT-login halves of this test.
    if std::env::var("CODEX_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return;
    }

    // Hermetic config isolation: setup reads the global config, so point it at
    // a throwaway dir. Integration tests cannot use the crate-private
    // `isolated_data_dir()` (`#[cfg(test)]`), so replicate it: point all four XDG
    // category overrides at a throwaway dir. This test lives in the serial
    // binary, which runs one test at a time, so mutating process env is safe.
    let data_home = tempfile::tempdir().unwrap();
    let _iso: Vec<EnvVar> = ISOLATED_ENV_VARS
        .iter()
        .copied()
        .map(|key| EnvVar::set(key, &data_home.path().to_string_lossy()))
        .collect();

    const CHATGPT_AUTH: &str = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"x"}}"#;

    // --- ChatGPT login: stay native, write no proxy entries. Codex does
    // not read OPENAI_API_KEY for model traffic, so a key in the environment
    // must not turn the stored login into API-key mode.
    {
        let (home, codex) = codex_home_with_auth(CHATGPT_AUTH);
        let _codex_home = CodexHome::set(&codex);
        let _openai_key = EnvVar::set("OPENAI_API_KEY", "sk-unrelated");
        let (_listener, port) = dummy_proxy_port();
        sobre::proxy_setup::install_proxy_env_unchecked(home.path(), port, true, false);

        let cfg = std::fs::read_to_string(codex.join("config.toml")).unwrap();
        assert!(
            !cfg.contains("model_provider = \"sobre-chatgpt\""),
            "a ChatGPT login must stay native — no model_provider pin (#597), got:\n{cfg}"
        );
        assert!(
            !cfg.contains("chatgpt_base_url"),
            "a ChatGPT login must write no proxy rail, got:\n{cfg}"
        );
        assert!(
            !cfg.contains("openai_base_url"),
            "a ChatGPT login must never use the OpenAI API-key /v1 rail, got:\n{cfg}"
        );
        assert!(
            cfg.contains("model = \"gpt-5.5\""),
            "unrelated Codex config must be preserved, got:\n{cfg}"
        );
    }

    // --- API-key login: Codex is pointed at the proxy via top-level openai_base_url.
    {
        let (home, codex) =
            codex_home_with_auth(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-test"}"#);
        let _codex_home = CodexHome::set(&codex);
        let (_listener, port) = dummy_proxy_port();
        sobre::proxy_setup::install_proxy_env_unchecked(home.path(), port, true, false);

        let cfg = std::fs::read_to_string(codex.join("config.toml")).unwrap();
        assert!(
            cfg.contains(&format!("openai_base_url = \"http://127.0.0.1:{port}/v1\"")),
            "API-key Codex must be pointed at the proxy via top-level openai_base_url, got:\n{cfg}"
        );
        assert!(
            cfg.contains("model = \"gpt-5.5\""),
            "unrelated Codex config must be preserved, got:\n{cfg}"
        );
    }
}
