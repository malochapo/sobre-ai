//! E2E tests for shell detection (`$SHELL`),
//! agent init, Windows path handling,
//! and pipe-guard (stdout not a terminal → bypass lean-ctx).
// noqa: SIZE_OK — single-responsibility E2E test module for the CLI binary.

use std::io::Write;
use std::process::{Command, Stdio};

fn lean_ctx_bin() -> String {
    env!("CARGO_BIN_EXE_lean-ctx").to_string()
}

fn run_with_env(
    args: &[&str],
    env_vars: &[(&str, &str)],
    stdin_data: Option<&str>,
) -> (String, String, i32) {
    let mut cmd = Command::new(lean_ctx_bin());
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("failed to spawn lean-ctx");

    if let Some(data) = stdin_data {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(data.as_bytes())
            .unwrap();
    }

    let output = child.wait_with_output().expect("failed to wait");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(1);
    (stdout, stderr, code)
}

fn run_hook_test(
    args: &[&str],
    env_vars: &[(&str, &str)],
    stdin_data: Option<&str>,
) -> (String, String, i32) {
    let mut cmd = Command::new(lean_ctx_bin());
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("failed to spawn lean-ctx");

    if let Some(data) = stdin_data {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(data.as_bytes())
            .unwrap();
    }

    let output = child.wait_with_output().expect("failed to wait");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(1);
    (stdout, stderr, code)
}

#[test]
fn posix_shell_from_env_is_used() {
    if cfg!(windows) {
        return; // /bin/sh not available on Windows
    }
    let (stdout, _stderr, code) = run_with_env(
        &["-c", "echo lean_ctx_shell_works"],
        &[("SHELL", "/bin/sh")],
        None,
    );
    assert_eq!(code, 0, "should succeed with /bin/sh");
    assert!(
        stdout.contains("lean_ctx_shell_works"),
        "should see echo output: {stdout}"
    );
}

#[test]
fn bash_from_env_is_used() {
    if !std::path::Path::new("/bin/bash").exists() {
        return;
    }
    let (stdout, _stderr, code) = run_with_env(
        &["-c", "echo $BASH_VERSION"],
        &[("SHELL", "/bin/bash")],
        None,
    );
    assert_eq!(code, 0, "should succeed with /bin/bash");
    assert!(!stdout.trim().is_empty(), "BASH_VERSION should be set");
}

#[test]
fn shell_exec_pipe_command() {
    if cfg!(windows) {
        return; // head -1 not available on Windows
    }
    let (stdout, _stderr, code) =
        run_with_env(&["-c", "echo 'line1\nline2\nline3' | head -1"], &[], None);
    assert_eq!(code, 0, "pipe should work");
    assert!(!stdout.trim().is_empty(), "should have output: {stdout}");
}

#[test]
fn shell_exec_and_chain() {
    let (stdout, _stderr, code) = run_with_env(&["-c", "echo first && echo second"], &[], None);
    assert_eq!(code, 0, "&& chain should work");
    assert!(stdout.contains("first"), "first: {stdout}");
    assert!(stdout.contains("second"), "second: {stdout}");
}

#[test]
fn shell_exec_semicolon_chain() {
    let (stdout, _stderr, code) = run_with_env(&["-c", "echo aaa; echo bbb"], &[], None);
    assert_eq!(code, 0, "; chain should work");
    assert!(stdout.contains("aaa"), "aaa: {stdout}");
    assert!(stdout.contains("bbb"), "bbb: {stdout}");
}

#[test]
fn shell_exec_subshell() {
    if cfg!(windows) {
        return; // $(...) subshell syntax varies on Windows
    }
    let (stdout, _stderr, code) = run_with_env(&["-c", "echo $(echo subshell_output)"], &[], None);
    assert_eq!(code, 0, "subshell should work");
    assert!(stdout.contains("subshell_output"), "subshell: {stdout}");
}

#[test]
fn shell_exec_env_var_expansion() {
    if cfg!(windows) {
        return; // $VAR syntax is bash-only; PowerShell uses $env:VAR
    }
    let (stdout, _stderr, code) = run_with_env(
        &["-c", "echo $TEST_LEAN_CTX_VAR"],
        &[("TEST_LEAN_CTX_VAR", "expanded_value")],
        None,
    );
    assert_eq!(code, 0);
    assert!(
        stdout.contains("expanded_value"),
        "env var expansion: {stdout}"
    );
}

#[test]
fn shell_exec_quoted_args() {
    let (stdout, _stderr, code) =
        run_with_env(&["-c", r#"echo "hello world with spaces""#], &[], None);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("hello world with spaces"),
        "quoted args: {stdout}"
    );
}

#[test]
fn agent_init_unknown_agent_fails() {
    let (_stdout, stderr, code) =
        run_with_env(&["init", "--agent", "nonexistent_agent"], &[], None);
    assert_ne!(code, 0, "unknown agent should fail");
    assert!(
        stderr.contains("Unknown agent"),
        "should say unknown: {stderr}"
    );
}

#[test]
fn hook_compress_replaces_long_bash_output_with_a_labelled_copy() {
    let home = tempfile::tempdir().expect("tempdir");
    let home_s = home.path().to_string_lossy().to_string();
    let stdout_text: String = (0..120)
        .map(|i| {
            format!(
                "commit {i:040x}\nAuthor: Dev <dev@example.com>\nDate:   Mon Jan 1 10:00:{:02} 2026 +0000\n\n    change number {i}\n\n",
                i % 60
            )
        })
        .collect::<Vec<_>>()
        .concat();
    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "git log" },
        "tool_response": { "stdout": stdout_text, "stderr": "", "interrupted": false, "isImage": false }
    })
    .to_string();
    let (stdout, stderr, code) = run_hook_test(
        &["hook", "compress"],
        &[("HOME", &home_s), ("LEAN_CTX_DATA_DIR", &home_s)],
        Some(&input),
    );
    assert_eq!(code, 0, "hook compress must not fail: {stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should contain hook JSON");
    let updated = &parsed["hookSpecificOutput"]["updatedToolOutput"];
    assert!(
        updated["stdout"]
            .as_str()
            .is_some_and(|s| s.contains("[sobre: output compressed")),
        "compressed output must say so: {parsed}"
    );
    assert!(
        parsed.get("decision").is_none() && !stdout.contains("permissionDecision"),
        "a post-run hook never decides anything: {stdout}"
    );
}

#[test]
fn hook_cursor_wraps_known_commands_without_a_permission_decision() {
    let home = tempfile::tempdir().expect("tempdir");
    let home_s = home.path().to_string_lossy().to_string();
    let input = r#"{"tool_name":"Shell","tool_input":{"command":"git status"}}"#;
    let (stdout, stderr, code) = run_hook_test(
        &["hook", "cursor"],
        &[("HOME", &home_s), ("LEAN_CTX_DATA_DIR", &home_s)],
        Some(input),
    );
    assert_eq!(code, 0, "hook cursor must not fail: {stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should contain hook JSON");
    let command = parsed["updated_input"]["command"]
        .as_str()
        .expect("updated command");
    assert!(command.contains("-c") && command.contains("git status"));
    assert!(
        !stdout.contains("permission"),
        "sobre never emits a permission decision: {stdout}"
    );

    // A Cursor rule naming the command keeps it as written.
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
    std::fs::write(
        home.path().join(".cursor/cli-config.json"),
        r#"{"permissions":{"allow":["Shell(git diff)"]}}"#,
    )
    .unwrap();
    // A different command: Cursor double-fires, so a repeated command replays
    // the first reply for a few seconds (#1032).
    let (stdout, _stderr, _code) = run_hook_test(
        &["hook", "cursor"],
        &[("HOME", &home_s), ("LEAN_CTX_DATA_DIR", &home_s)],
        Some(r#"{"tool_name":"Shell","tool_input":{"command":"git diff"}}"#),
    );
    assert_eq!(
        stdout.trim(),
        "{}",
        "a host rule on the command blocks the rewrite"
    );
}

#[test]
fn agent_init_codex_installs_hooks_and_skill() {
    let tmpdir = tempfile::tempdir().expect("create tempdir");
    let home = tmpdir.path();
    let codex_dir = home.join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();

    let home_str = home.to_string_lossy().to_string();
    #[cfg(not(windows))]
    let envs = vec![("HOME", home_str.as_str())];
    #[cfg(windows)]
    let envs = vec![
        ("HOME", home_str.as_str()),
        ("USERPROFILE", home_str.as_str()),
    ];

    let (_stdout, stderr, code) = run_with_env(&["init", "--agent", "codex"], &envs, None);
    assert_eq!(code, 0, "codex init should succeed: {stderr}");

    for name in ["AGENTS.md", "LEAN-CTX.md"] {
        assert!(
            !codex_dir.join(name).exists(),
            "init must not write {name}: rules travel in the MCP instructions"
        );
    }
    assert!(
        codex_dir.join("skills/sobre/SKILL.md").exists(),
        "init installs the sobre skill for Codex"
    );

    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(codex_dir.join("hooks.json")).unwrap())
            .expect("hooks.json should be valid");
    assert!(
        hooks["hooks"].get("PreToolUse").is_none(),
        "no pre-run hook for Codex: it would require an allow decision"
    );
    assert!(hooks.to_string().contains(" hook observe"));
    assert!(
        !hooks.to_string().contains("codex-session-start"),
        "no session-start rules hook: rules travel in the MCP instructions"
    );

    let config = std::fs::read_to_string(codex_dir.join("config.toml")).unwrap_or_default();
    assert!(
        config.contains("hooks = true"),
        "init should enable Codex hook support"
    );
}

#[test]
fn piped_output_is_not_compressed() {
    if cfg!(windows) {
        return;
    }
    let bin = lean_ctx_bin();
    let script = r#"echo "line one"; echo "line two"; echo "line three""#.to_string();
    let output = Command::new(&bin)
        .args(["-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("line one"),
        "piped output must contain original content: {stdout}"
    );
}

#[test]
fn lean_ctx_c_preserves_output_when_piped() {
    if cfg!(windows) {
        return;
    }
    let bin = lean_ctx_bin();

    let output = Command::new(&bin)
        .args(["-c", "echo MARKER_STRING_12345"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run lean-ctx -c echo");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("MARKER_STRING_12345"),
        "lean-ctx -c must preserve output content when piped: {stdout}"
    );
}

#[test]
fn lean_ctx_c_multiline_preserves_all_lines_when_piped() {
    if cfg!(windows) {
        return;
    }
    let bin = lean_ctx_bin();
    let cmd = "echo LINE_A && echo LINE_B && echo LINE_C";
    let output = Command::new(&bin)
        .args(["-c", cmd])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("LINE_A"), "LINE_A: {stdout}");
    assert!(stdout.contains("LINE_B"), "LINE_B: {stdout}");
    assert!(stdout.contains("LINE_C"), "LINE_C: {stdout}");
}
