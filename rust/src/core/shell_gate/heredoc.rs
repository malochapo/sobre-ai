/// Strip the *bodies* of quoted-delimiter heredocs (`<<'EOF' … EOF`,
/// `<<-"E" … E`) prior to allowlist analysis (#876).
///
/// A quoted heredoc delimiter disables all shell expansion, so every body line
/// is pure literal stdin data — never an executable command. Left in place, the
/// operator-splitter dices those lines into "segments" and blocks the first word
/// that isn't allowlisted (e.g. a commit message piped via `git commit -F -`,
/// whose first token is `feat(...)`).
///
/// Only quoted delimiters are stripped. An *unquoted* `<<EOF` heredoc DOES expand
/// `$()`/backticks/`$VAR` in its body, so those bodies are deliberately left
/// intact for the command-substitution checks to see.
pub(crate) fn strip_quoted_heredoc_bodies(command: &str) -> String {
    if !command.contains("<<") {
        return command.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    // Delimiters awaiting their terminator line, in body order (stacked heredocs
    // `cmd <<'A' <<'B'` drain A's body first, then B's).
    let mut pending: Vec<String> = Vec::new();
    for line in command.lines() {
        if pending.is_empty() {
            out.push(line);
            pending = heredoc_delims(line, true);
        } else if line.trim_start_matches('\t').trim() == pending[0] {
            // Terminator line: drop it and resume. `<<-` allows leading tabs; be
            // lenient (over-stripping body data is harmless — a heredoc body is
            // never a command anyway).
            pending.remove(0);
        }
        // else: a heredoc body line — dropped (not pushed to `out`).
    }
    out.join("\n")
}

/// Like `strip_quoted_heredoc_bodies` but strips bodies for **all** heredocs
/// (quoted *and* unquoted delimiters). Use for checks that must never interpret
/// heredoc body content as commands or redirects (#931).
pub fn strip_all_heredoc_bodies(command: &str) -> String {
    if !command.contains("<<") {
        return command.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for line in command.lines() {
        if pending.is_empty() {
            out.push(line);
            pending = heredoc_delims(line, false);
        } else if line.trim_start_matches('\t').trim() == pending[0] {
            pending.remove(0);
        }
    }
    out.join("\n")
}

/// Scan one line for heredoc operators with a **quoted** delimiter and return
/// their bare delimiter names in source order. Quote-aware, so a `<<` inside a
/// quoted string is ignored; a `<<<` here-string (no body) is skipped.
pub(crate) fn heredoc_delims(line: &str, quoted_only: bool) -> Vec<String> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut delims = Vec::new();
    while i < len {
        let ch = bytes[i];
        if in_single {
            if ch == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            match ch {
                b'\\' => i = (i + 2).min(len),
                b'"' => {
                    in_double = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        match ch {
            b'\\' => i = (i + 2).min(len),
            b'\'' => {
                in_single = true;
                i += 1;
            }
            b'"' => {
                in_double = true;
                i += 1;
            }
            b'<' if i + 1 < len && bytes[i + 1] == b'<' => {
                // `<<<` is a here-string (no body), not a heredoc.
                if i + 2 < len && bytes[i + 2] == b'<' {
                    i += 3;
                    continue;
                }
                let mut j = i + 2;
                if j < len && bytes[j] == b'-' {
                    j += 1; // `<<-` (tab-stripped terminator)
                }
                while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if let Some((delim, quoted, next)) = read_heredoc_delim(bytes, j) {
                    if !quoted_only || quoted {
                        delims.push(delim);
                    }
                    i = next;
                    continue;
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    delims
}

/// Parse a heredoc delimiter token starting at `start`, returning its bare name
/// (quotes/escapes removed), whether any part was quoted, and the index just
/// past the token. `None` when no delimiter is present.
pub(crate) fn read_heredoc_delim(bytes: &[u8], start: usize) -> Option<(String, bool, usize)> {
    let len = bytes.len();
    let mut i = start;
    let mut name: Vec<u8> = Vec::new();
    let mut quoted = false;
    while i < len {
        match bytes[i] {
            b'\'' => {
                quoted = true;
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    name.push(bytes[i]);
                    i += 1;
                }
                i += usize::from(i < len); // skip closing quote if present
            }
            b'"' => {
                quoted = true;
                i += 1;
                while i < len && bytes[i] != b'"' {
                    name.push(bytes[i]);
                    i += 1;
                }
                i += usize::from(i < len);
            }
            b'\\' => {
                quoted = true;
                i += 1;
                if i < len {
                    name.push(bytes[i]);
                    i += 1;
                }
            }
            b' ' | b'\t' | b'<' | b'>' | b'|' | b'&' | b';' => break,
            c => {
                name.push(c);
                i += 1;
            }
        }
    }
    if name.is_empty() {
        None
    } else {
        Some((String::from_utf8_lossy(&name).into_owned(), quoted, i))
    }
}

/// Strip shell comments (`#` to end-of-line) so the allowlist tokenizer never
/// mistakes a comment for a command (#1109).
///
/// Per POSIX, `#` only starts a comment when it is unquoted and begins a word:
/// at the start of the command, or immediately after whitespace or one of the
/// unquoted metacharacters `;`, `&`, `|`, `(`. Anywhere else the `#` is part of
/// a word, so this deliberately leaves intact:
///   - `#` inside single or double quotes (`echo "# not a comment"`),
///   - parameter expansions like `${#arr}` and `${var#prefix}`,
///   - arithmetic bases like `$((16#ff))`,
///   - URLs / fragments like `http://host/path#frag`,
///   - an escaped `\#`.
///
/// Run this AFTER heredoc bodies are stripped: a body line may legitimately
/// contain `#`, and once the body is gone it can't be misread here.
pub(crate) fn strip_comments(command: &str) -> String {
    if !command.contains('#') {
        return command.to_string();
    }
    let bytes = command.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut in_single = false;
    let mut in_double = false;
    // The start of the command counts as a word boundary.
    let mut at_boundary = true;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_single {
            out.push(c);
            if c == b'\'' {
                in_single = false;
            }
            at_boundary = false;
            i += 1;
            continue;
        }
        if in_double {
            // A backslash inside double quotes escapes the next byte; copy both
            // so a `\"` doesn't prematurely close the quote.
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(c);
                out.push(bytes[i + 1]);
                at_boundary = false;
                i += 2;
                continue;
            }
            out.push(c);
            if c == b'"' {
                in_double = false;
            }
            at_boundary = false;
            i += 1;
            continue;
        }
        match c {
            b'\'' => {
                in_single = true;
                out.push(c);
                at_boundary = false;
                i += 1;
            }
            b'"' => {
                in_double = true;
                out.push(c);
                at_boundary = false;
                i += 1;
            }
            b'\\' => {
                // An unquoted backslash escapes the next byte, so `\#` is a
                // literal `#`, never a comment.
                out.push(c);
                if i + 1 < bytes.len() {
                    out.push(bytes[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
                at_boundary = false;
            }
            b'#' if at_boundary => {
                // Comment: drop everything up to (but not including) the newline.
                while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                    i += 1;
                }
                // `at_boundary` stays true; the newline (if any) is emitted next.
            }
            b'\n' | b'\r' | b' ' | b'\t' | b';' | b'&' | b'|' | b'(' => {
                out.push(c);
                at_boundary = true;
                i += 1;
            }
            _ => {
                out.push(c);
                at_boundary = false;
                i += 1;
            }
        }
    }
    // Every non-ASCII (multi-byte) byte is copied verbatim through the `_` arm,
    // so the result is always valid UTF-8; fall back to the input if not.
    String::from_utf8(out).unwrap_or_else(|_| command.to_string())
}
