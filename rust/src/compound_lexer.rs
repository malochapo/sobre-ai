//! Splits a shell line at its operators. Hand-written: no crate splits a shell
//! line while keeping the operators (`shlex` and `shell-words` return the words
//! and drop `&&`, `||`, `;` and `|`, which a rewrite has to emit back as they
//! were).

/// Splits a compound shell command into segments separated by `&&`, `||`, `;`.
/// Pipes (`|`) are treated specially: only the left side of a pipe is eligible
/// for rewriting (the right side consumes output format and must stay raw).
///
/// Respects single quotes, double quotes, backtick-quotes, and `$(...)` subshells
/// so that operators inside quoted strings are not treated as separators.
///
/// Returns a `Vec<Segment>` where each entry is either a command segment or an
/// operator token that should be emitted verbatim.

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Command(String),
    Operator(String),
}

pub fn split_compound(input: &str) -> Vec<Segment> {
    let input = input.trim();
    if input.is_empty() {
        return vec![];
    }

    if contains_heredoc(input) {
        return vec![Segment::Command(input.to_string())];
    }

    let chars: Vec<char> = input.chars().collect();
    let mut segments: Vec<Segment> = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    let len = chars.len();

    while i < len {
        let ch = chars[i];

        match ch {
            '\'' => {
                current.push(ch);
                i += 1;
                while i < len && chars[i] != '\'' {
                    current.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    current.push('\'');
                    i += 1;
                }
            }
            '"' => {
                current.push(ch);
                i += 1;
                while i < len && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < len {
                        current.push('\\');
                        current.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    current.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    current.push('"');
                    i += 1;
                }
            }
            '`' => {
                current.push(ch);
                i += 1;
                while i < len && chars[i] != '`' {
                    current.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    current.push('`');
                    i += 1;
                }
            }
            '$' if i + 1 < len && chars[i + 1] == '(' => {
                current.push('$');
                current.push('(');
                i += 2;
                let mut depth = 1;
                while i < len && depth > 0 {
                    if chars[i] == '(' {
                        depth += 1;
                    } else if chars[i] == ')' {
                        depth -= 1;
                    }
                    current.push(chars[i]);
                    i += 1;
                }
            }
            '\\' if i + 1 < len => {
                current.push('\\');
                current.push(chars[i + 1]);
                i += 2;
            }
            '&' if i + 1 < len && chars[i + 1] == '&' => {
                push_command(&mut segments, &current);
                current.clear();
                segments.push(Segment::Operator("&&".to_string()));
                i += 2;
            }
            '|' if i + 1 < len && chars[i + 1] == '|' => {
                push_command(&mut segments, &current);
                current.clear();
                segments.push(Segment::Operator("||".to_string()));
                i += 2;
            }
            '|' => {
                push_command(&mut segments, &current);
                current.clear();
                segments.push(Segment::Operator("|".to_string()));
                let rest: String = chars[i + 1..].iter().collect::<String>();
                let rest = rest.trim().to_string();
                if !rest.is_empty() {
                    segments.push(Segment::Command(rest));
                }
                return segments;
            }
            ';' => {
                push_command(&mut segments, &current);
                current.clear();
                segments.push(Segment::Operator(";".to_string()));
                i += 1;
            }
            _ => {
                current.push(ch);
                i += 1;
            }
        }
    }

    push_command(&mut segments, &current);
    segments
}

fn push_command(segments: &mut Vec<Segment>, cmd: &str) {
    let trimmed = cmd.trim();
    if !trimmed.is_empty() {
        segments.push(Segment::Command(trimmed.to_string()));
    }
}

fn contains_heredoc(input: &str) -> bool {
    input.contains("<<") || input.contains("$((")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_command() {
        let segs = split_compound("git status");
        assert_eq!(segs, vec![Segment::Command("git status".into())]);
    }

    #[test]
    fn and_chain() {
        let segs = split_compound("cd src && git status && echo done");
        assert_eq!(
            segs,
            vec![
                Segment::Command("cd src".into()),
                Segment::Operator("&&".into()),
                Segment::Command("git status".into()),
                Segment::Operator("&&".into()),
                Segment::Command("echo done".into()),
            ]
        );
    }

    #[test]
    fn pipe_stops_at_right() {
        let segs = split_compound("git log --oneline | grep fix");
        assert_eq!(
            segs,
            vec![
                Segment::Command("git log --oneline".into()),
                Segment::Operator("|".into()),
                Segment::Command("grep fix".into()),
            ]
        );
    }

    #[test]
    fn pipe_in_chain() {
        let segs = split_compound("cd src && git log | head -5");
        assert_eq!(
            segs,
            vec![
                Segment::Command("cd src".into()),
                Segment::Operator("&&".into()),
                Segment::Command("git log".into()),
                Segment::Operator("|".into()),
                Segment::Command("head -5".into()),
            ]
        );
    }

    #[test]
    fn semicolons() {
        let segs = split_compound("git add .; git commit -m 'fix'");
        assert_eq!(
            segs,
            vec![
                Segment::Command("git add .".into()),
                Segment::Operator(";".into()),
                Segment::Command("git commit -m 'fix'".into()),
            ]
        );
    }

    #[test]
    fn or_chain() {
        let segs = split_compound("git pull || echo failed");
        assert_eq!(
            segs,
            vec![
                Segment::Command("git pull".into()),
                Segment::Operator("||".into()),
                Segment::Command("echo failed".into()),
            ]
        );
    }

    #[test]
    fn quoted_ampersand_not_split() {
        let segs = split_compound("echo 'foo && bar'");
        assert_eq!(segs, vec![Segment::Command("echo 'foo && bar'".into())]);
    }

    #[test]
    fn double_quoted_pipe_not_split() {
        let segs = split_compound(r#"echo "hello | world""#);
        assert_eq!(
            segs,
            vec![Segment::Command(r#"echo "hello | world""#.into())]
        );
    }

    #[test]
    fn heredoc_kept_whole() {
        let segs = split_compound("cat <<EOF\nhello\nEOF && echo done");
        assert_eq!(
            segs,
            vec![Segment::Command(
                "cat <<EOF\nhello\nEOF && echo done".into()
            )]
        );
    }

    #[test]
    fn subshell_not_split() {
        let segs = split_compound("echo $(git status && echo ok)");
        assert_eq!(
            segs,
            vec![Segment::Command("echo $(git status && echo ok)".into())]
        );
    }
}
