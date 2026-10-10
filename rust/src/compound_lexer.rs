//! Splits a shell line into the commands it runs. Hand-written: `shlex` and
//! `shell-words` split a line into words but keep an unquoted `a&&b` or
//! `a&b` as one word, so they cannot say whether a line chains, pipes or
//! backgrounds commands.

/// One command of a shell line, as [`commands`] splits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command<'a> {
    /// The command, trimmed.
    pub text: &'a str,
    /// True when it reads the previous command's output (`|` or `|&`).
    pub piped: bool,
}

/// True when `input` holds a control operator outside quotes, backticks and
/// `$(...)`: `&&`, `||`, `|`, `;`, a background `&`, or a newline. An `&`
/// that belongs to a redirection (`2>&1`, `&>out`, `<&3`) is not one.
pub fn has_control_operator(input: &str) -> bool {
    !operators(input.trim()).is_empty()
}

/// The commands of `input`, split at the control operators
/// [`has_control_operator`] finds. Quotes, backticks and `$(...)` stay in the
/// command that holds them; empty commands (a trailing `;` or `&`) are left
/// out.
pub fn commands(input: &str) -> Vec<Command<'_>> {
    let line = input.trim();
    let mut out = Vec::new();
    let mut start = 0;
    let mut piped = false;
    for (from, to, pipe) in operators(line) {
        out.push(Command {
            text: line[start..from].trim(),
            piped,
        });
        start = to;
        piped = pipe;
    }
    out.push(Command {
        text: line[start..].trim(),
        piped,
    });
    out.retain(|c| !c.text.is_empty());
    out
}

/// Each control operator of `line`: its byte range, and whether it is a pipe.
/// Scans bytes: every character it looks for is ASCII, and UTF-8 never uses
/// an ASCII byte inside a multi-byte character.
fn operators(line: &str) -> Vec<(usize, usize, bool)> {
    let b = line.as_bytes();
    let len = b.len();
    let mut ops = Vec::new();
    let mut i = 0;

    while i < len {
        match b[i] {
            b'\'' | b'`' => {
                let close = b[i];
                i += 1;
                while i < len && b[i] != close {
                    i += 1;
                }
                i += 1;
            }
            b'"' => {
                i += 1;
                while i < len && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'$' if i + 1 < len && b[i + 1] == b'(' => {
                i += 2;
                let mut depth = 1;
                while i < len && depth > 0 {
                    match b[i] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'\\' => i += 2,
            b'&' => {
                let redirection =
                    (i > 0 && matches!(b[i - 1], b'>' | b'<')) || (i + 1 < len && b[i + 1] == b'>');
                if redirection {
                    i += 1;
                } else {
                    let to = if i + 1 < len && b[i + 1] == b'&' { i + 2 } else { i + 1 };
                    ops.push((i, to, false));
                    i = to;
                }
            }
            b'|' => {
                let (to, pipe) = match b.get(i + 1) {
                    Some(b'|') => (i + 2, false),
                    Some(b'&') => (i + 2, true),
                    _ => (i + 1, true),
                };
                ops.push((i, to, pipe));
                i = to;
            }
            b';' | b'\n' => {
                ops.push((i, i + 1, false));
                i += 1;
            }
            _ => i += 1,
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::{Command, commands, has_control_operator};

    #[test]
    fn a_single_command_has_none() {
        for cmd in [
            "git status",
            "cargo test --lib",
            "git commit -m 'fix'",
            "",
            "cargo build 2>&1",
            "cargo build &>out.log",
            "exec 3<&0",
        ] {
            assert!(!has_control_operator(cmd), "{cmd}");
        }
    }

    #[test]
    fn chains_pipes_and_background_jobs_have_one() {
        for cmd in [
            "cd src && git status",
            "git pull || echo failed",
            "git log --oneline | grep fix",
            "git add .; git commit -m 'fix'",
            "git status & rm -rf target",
            "npm run dev &",
            "git status&rm -rf target",
            "cargo build |& tee log",
            "git status\nrm -rf target",
        ] {
            assert!(has_control_operator(cmd), "{cmd}");
        }
    }

    #[test]
    fn operators_inside_quotes_and_substitutions_do_not_count() {
        for cmd in [
            "echo 'foo && bar'",
            r#"echo "hello | world; & done""#,
            r#"echo "a \" && b""#,
            "echo `a && b`",
            "echo $(git status && echo ok)",
            "echo $( (a; b) )",
            r"echo a\;b \&c",
        ] {
            assert!(!has_control_operator(cmd), "{cmd}");
        }
    }

    #[test]
    fn commands_split_at_every_operator_and_know_which_read_a_pipe() {
        let c = |text, piped| Command { text, piped };
        assert_eq!(commands("git status"), vec![c("git status", false)]);
        assert_eq!(
            commands("cd /repo && sed -n 1,80p a.py; grep -A8 'def f' b.py"),
            vec![
                c("cd /repo", false),
                c("sed -n 1,80p a.py", false),
                c("grep -A8 'def f' b.py", false),
            ]
        );
        assert_eq!(
            commands("cat a.py | head -50 || echo 'a | b'"),
            vec![
                c("cat a.py", false),
                c("head -50", true),
                c("echo 'a | b'", false),
            ]
        );
        assert_eq!(
            commands("cargo test 2>&1 |& tail -5 &"),
            vec![c("cargo test 2>&1", false), c("tail -5", true)]
        );
        assert_eq!(
            commands("python3 x.py\nsed -n 1,5p é.py"),
            vec![c("python3 x.py", false), c("sed -n 1,5p é.py", false)]
        );
        assert_eq!(commands("  ;  "), Vec::<Command>::new());
    }
}
