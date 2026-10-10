//! Tells whether a shell line runs more than one command. Hand-written:
//! `shlex` and `shell-words` split a line into words but keep an unquoted
//! `a&&b` or `a&b` as one word, so they cannot say whether a line chains,
//! pipes or backgrounds commands.

/// True when `input` holds a control operator outside quotes, backticks and
/// `$(...)`: `&&`, `||`, `|`, `;`, a background `&`, or a newline. An `&`
/// that belongs to a redirection (`2>&1`, `&>out`, `<&3`) is not one.
pub fn has_control_operator(input: &str) -> bool {
    let chars: Vec<char> = input.trim().chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        match chars[i] {
            '\'' | '`' => {
                let close = chars[i];
                i += 1;
                while i < len && chars[i] != close {
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                i += 1;
                while i < len && chars[i] != '"' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            }
            '$' if i + 1 < len && chars[i + 1] == '(' => {
                i += 2;
                let mut depth = 1;
                while i < len && depth > 0 {
                    match chars[i] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            '\\' => i += 2,
            '&' => {
                let redirection = (i > 0 && matches!(chars[i - 1], '>' | '<'))
                    || (i + 1 < len && chars[i + 1] == '>');
                if !redirection {
                    return true;
                }
                i += 1;
            }
            '|' | ';' | '\n' => return true,
            _ => i += 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::has_control_operator;

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
}
