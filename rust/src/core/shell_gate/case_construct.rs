use crate::core::error::ShellError;

/// Replace each `case` construct with all of its arm bodies. Pattern labels
/// select which body runs, but they are not commands and must not enter the
/// allowlist walk. Including every body preserves deny-by-default validation:
/// every command that could execute for any pattern must be allowlisted.
pub(crate) fn rewrite_case_constructs(command: &str) -> Result<String, ShellError> {
    let Some(start) = find_shell_word(command, "case", 0) else {
        return Ok(command.to_string());
    };
    let (replacement, consumed) = parse_case_construct(&command[start..])?;
    let mut rewritten = String::with_capacity(command.len());
    rewritten.push_str(&command[..start]);
    rewritten.push_str(&replacement);
    rewritten.push_str(&rewrite_case_constructs(&command[start + consumed..])?);
    Ok(rewritten)
}

/// Parse one case statement beginning at `case`, returning its arm bodies and
/// the byte length consumed through the matching `esac`.
pub(super) fn parse_case_construct(input: &str) -> Result<(String, usize), ShellError> {
    let in_start = find_shell_word(input, "in", 4)
        .ok_or_else(|| case_parse_error(input, "missing `in` after the case expression"))?;
    let mut cursor = in_start + 2;
    let mut bodies = Vec::new();
    let end = loop {
        cursor = skip_shell_whitespace(input, cursor);
        if is_shell_word_at(input, cursor, "esac") {
            break cursor + 4;
        }
        let pattern_end = find_case_pattern_end(input, cursor)
            .ok_or_else(|| case_parse_error(input, "missing `)` after a case pattern"))?;
        let body_start = pattern_end + 1;
        match find_case_arm_end(input, body_start) {
            Some(CaseArmEnd::Terminator { body_end, next }) => {
                bodies.push(input[body_start..body_end].trim());
                cursor = next;
            }
            Some(CaseArmEnd::Esac {
                body_end,
                esac_start,
            }) => {
                bodies.push(input[body_start..body_end].trim());
                break esac_start + 4;
            }
            None => {
                return Err(case_parse_error(
                    input,
                    "missing `;;` arm terminator or matching `esac`",
                ));
            }
        }
    };

    let mut replacement = String::new();
    for body in bodies {
        if body.is_empty() {
            continue;
        }
        let body = rewrite_case_constructs(body)?;
        if !replacement.is_empty() {
            replacement.push_str("; ");
        }
        replacement.push_str(body.trim());
    }
    Ok((replacement, end))
}

#[derive(Debug, Clone, Copy)]
pub(super) enum CaseArmEnd {
    Terminator { body_end: usize, next: usize },
    Esac { body_end: usize, esac_start: usize },
}

/// Find the end of a case pattern, ignoring quoted and nested shell syntax.
pub(super) fn find_case_pattern_end(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut i = start;
    let mut paren_depth = 0u32;
    let mut brace_depth = 0u32;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        if in_single {
            if bytes[i] == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
            } else {
                if bytes[i] == b'"' {
                    in_double = false;
                }
                i += 1;
            }
            continue;
        }
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' => {
                in_single = true;
                i += 1;
            }
            b'"' => {
                in_double = true;
                i += 1;
            }
            b'$' if bytes.get(i + 1) == Some(&b'{') || bytes.get(i + 1) == Some(&b'(') => {
                i = skip_shell_expansion(bytes, i).unwrap_or(bytes.len());
            }
            b'(' => {
                paren_depth += 1;
                i += 1;
            }
            b')' if paren_depth > 0 => {
                paren_depth -= 1;
                i += 1;
            }
            b')' => return Some(i),
            b'{' => {
                brace_depth += 1;
                i += 1;
            }
            b'}' if brace_depth > 0 => {
                brace_depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Find the next top-level `;;` or the matching outer `esac` in an arm body.
pub(super) fn find_case_arm_end(input: &str, start: usize) -> Option<CaseArmEnd> {
    let bytes = input.as_bytes();
    let mut i = start;
    let mut paren_depth = 0u32;
    let mut brace_depth = 0u32;
    let mut nested_cases = 0u32;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        if in_single {
            if bytes[i] == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
            } else {
                if bytes[i] == b'"' {
                    in_double = false;
                }
                i += 1;
            }
            continue;
        }
        if paren_depth == 0 && brace_depth == 0 {
            if is_shell_word_at(input, i, "case") {
                nested_cases += 1;
                i += 4;
                continue;
            }
            if is_shell_word_at(input, i, "esac") {
                if nested_cases == 0 {
                    return Some(CaseArmEnd::Esac {
                        body_end: i,
                        esac_start: i,
                    });
                }
                nested_cases -= 1;
                i += 4;
                continue;
            }
        }
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' => {
                in_single = true;
                i += 1;
            }
            b'"' => {
                in_double = true;
                i += 1;
            }
            b'$' if bytes.get(i + 1) == Some(&b'{') || bytes.get(i + 1) == Some(&b'(') => {
                i = skip_shell_expansion(bytes, i).unwrap_or(bytes.len());
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
            b';' if nested_cases == 0
                && paren_depth == 0
                && brace_depth == 0
                && bytes.get(i + 1) == Some(&b';') =>
            {
                return Some(CaseArmEnd::Terminator {
                    body_end: i,
                    next: i + 2,
                });
            }
            _ => i += 1,
        }
    }
    None
}

/// Return the first standalone unquoted shell word `word` at or after `start`.
pub(super) fn find_shell_word(input: &str, word: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let word_bytes = word.as_bytes();
    let mut i = start.min(bytes.len());
    while i + word_bytes.len() <= bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' => i = skip_quoted(bytes, i, b'\'').unwrap_or(bytes.len()),
            b'"' => i = skip_quoted(bytes, i, b'"').unwrap_or(bytes.len()),
            b'`' => i = skip_backticks(bytes, i).unwrap_or(bytes.len()),
            b'$' if bytes.get(i + 1) == Some(&b'{') || bytes.get(i + 1) == Some(&b'(') => {
                i = skip_shell_expansion(bytes, i).unwrap_or(bytes.len());
            }
            _ if bytes[i..].starts_with(word_bytes)
                && is_shell_word_boundary(bytes, i, word_bytes.len()) =>
            {
                return Some(i);
            }
            _ => i += 1,
        }
    }
    None
}

pub(super) fn is_shell_word_at(input: &str, start: usize, word: &str) -> bool {
    start + word.len() <= input.len()
        && input.as_bytes()[start..].starts_with(word.as_bytes())
        && is_shell_word_boundary(input.as_bytes(), start, word.len())
}

pub(super) fn is_shell_word_boundary(bytes: &[u8], start: usize, word_len: usize) -> bool {
    let boundary = |c: u8| {
        c.is_ascii_whitespace() || matches!(c, b';' | b'&' | b'|' | b'(' | b')' | b'{' | b'}')
    };
    (start == 0 || boundary(bytes[start - 1]))
        && (start + word_len == bytes.len() || boundary(bytes[start + word_len]))
}

pub(super) fn skip_shell_whitespace(input: &str, start: usize) -> usize {
    input
        .as_bytes()
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, c)| !c.is_ascii_whitespace())
        .map_or(input.len(), |(i, _)| i)
}

pub(super) fn skip_quoted(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && quote == b'"' {
            i = (i + 2).min(bytes.len());
        } else if bytes[i] == quote {
            return Some(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

pub(super) fn skip_backticks(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = (i + 2).min(bytes.len());
        } else if bytes[i] == b'`' {
            return Some(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

pub(super) fn skip_shell_expansion(bytes: &[u8], start: usize) -> Option<usize> {
    let (open, close) = match bytes.get(start..start + 2) {
        Some(b"${") => (b'{', b'}'),
        Some(b"$(") => (b'(', b')'),
        _ => return None,
    };
    let mut depth = 1u32;
    let mut i = start + 2;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = (i + 2).min(bytes.len());
            continue;
        }
        if bytes[i] == b'\'' {
            i = skip_quoted(bytes, i, b'\'').unwrap_or(bytes.len());
            continue;
        }
        if bytes[i] == b'"' {
            i = skip_quoted(bytes, i, b'"').unwrap_or(bytes.len());
            continue;
        }
        if bytes[i] == open {
            depth += 1;
        } else if bytes[i] == close {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
        i += 1;
    }
    None
}

pub(super) fn case_parse_error(input: &str, reason: &str) -> ShellError {
    format!(
        "[BLOCKED — DO NOT RETRY] `case` construct could not be safely parsed: {reason}.\n\\
         Command: {input}"
    )
    .into()
}

/// Quote-aware scan for a `;;` terminator (the `case` arm separator).
pub(crate) fn contains_double_semicolon(command: &str) -> bool {
    let bytes = command.as_bytes();
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
            if ch == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                in_double_quote = false;
            }
            i += 1;
            continue;
        }
        match ch {
            b'\'' => in_single_quote = true,
            b'"' => in_double_quote = true,
            b';' if i + 1 < len && bytes[i + 1] == b';' => return true,
            _ => {}
        }
        i += 1;
    }
    false
}
