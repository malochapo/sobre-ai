//! The binary end to end: `-c` with the user's `$SHELL` and common shell
//! syntax, `init` for an unknown agent and for Codex, and the `hook`
//! entrypoints the agents call.

use std::io::Write;
use std::process::{Command, Stdio};

fn sobre_bin() -> String {
    env!("CARGO_BIN_EXE_sobre").to_string()
}

fn run_with_env(
    args: &[&str],
    env_vars: &[(&str, &str)],
    stdin_data: Option<&str>,
) -> (String, String, i32) {
    let mut cmd = Command::new(sobre_bin());
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("failed to spawn sobre");

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

#[cfg(unix)]
#[test]
fn posix_shell_from_env_is_used() {
    let (stdout, _stderr, code) = run_with_env(
        &["-c", "echo sobre_shell_works"],
        &[("SHELL", "/bin/sh")],
        None,
    );
    assert_eq!(code, 0, "should succeed with /bin/sh");
    assert!(
        stdout.contains("sobre_shell_works"),
        "should see echo output: {stdout}"
    );
}

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn shell_exec_pipe_command() {
    let (stdout, _stderr, code) = run_with_env(
        &["-c", "printf 'line1\\nline2\\nline3\\n' | head -1"],
        &[],
        None,
    );
    assert_eq!(code, 0, "pipe should work");
    assert_eq!(
        stdout.trim(),
        "line1",
        "only the first line passes the pipe"
    );
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

#[cfg(unix)]
#[test]
fn shell_exec_subshell() {
    let (stdout, _stderr, code) = run_with_env(&["-c", "echo $(echo subshell_output)"], &[], None);
    assert_eq!(code, 0, "subshell should work");
    assert!(stdout.contains("subshell_output"), "subshell: {stdout}");
}

#[cfg(unix)]
#[test]
fn shell_exec_env_var_expansion() {
    let (stdout, _stderr, code) = run_with_env(
        &["-c", "echo $TEST_SOBRE_VAR"],
        &[("TEST_SOBRE_VAR", "expanded_value")],
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

/// A command a signal ends makes `sobre -c` exit `128 + signal`, as a shell
/// reports it: when sobre compresses the output, and when a parent sobre
/// already owns the command tree (`SOBRE_WRAPPED`) and it runs as written.
#[cfg(unix)]
#[test]
fn shell_exec_reports_a_signal_as_128_plus_the_signal() {
    for wrapped in [false, true] {
        let mut cmd = Command::new(sobre_bin());
        cmd.args(["-c", "kill -TERM $$"])
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null());
        if wrapped {
            cmd.env("SOBRE_WRAPPED", "1");
        } else {
            cmd.env_remove("SOBRE_WRAPPED");
        }
        let status = cmd.output().expect("failed to spawn sobre").status;
        assert_eq!(status.code(), Some(128 + 15), "wrapped: {wrapped}");
    }
}

#[test]
fn agent_init_unknown_agent_fails() {
    let (_stdout, stderr, code) =
        run_with_env(&["init", "--agent", "nonexistent_agent"], &[], None);
    assert_ne!(code, 0, "unknown agent should fail");
    assert!(
        stderr.contains("unknown agent"),
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
    let (stdout, stderr, code) = run_with_env(
        &["hook", "compress"],
        &[("HOME", &home_s), ("SOBRE_DATA_DIR", &home_s)],
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
    let (stdout, stderr, code) = run_with_env(
        &["hook", "cursor"],
        &[("HOME", &home_s), ("SOBRE_DATA_DIR", &home_s)],
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
    // the first reply for a few seconds.
    let (stdout, _stderr, _code) = run_with_env(
        &["hook", "cursor"],
        &[("HOME", &home_s), ("SOBRE_DATA_DIR", &home_s)],
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

    for name in ["AGENTS.md", "SOBRE.md"] {
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
