//! Terminal text helpers: the three ANSI attributes the CLI uses, switched off
//! under `NO_COLOR` or when stdout is not a terminal, and visible-width
//! measurement that skips escape sequences.

use std::io::IsTerminal;

fn no_color() -> bool {
    std::env::var("NO_COLOR").is_ok() || !std::io::stdout().is_terminal()
}

pub fn rst() -> &'static str {
    if no_color() { "" } else { "\x1b[0m" }
}

pub fn bold() -> &'static str {
    if no_color() { "" } else { "\x1b[1m" }
}

pub fn dim() -> &'static str {
    if no_color() { "" } else { "\x1b[2m" }
}

/// Visual width of a string in terminal columns, ignoring ANSI escape sequences
/// and accounting for wide characters (emoji, CJK = 2 columns).
pub fn visual_len(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut len = 0usize;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            len += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    len
}

/// Pad a string to `target` visual width with spaces on the right.
/// If the string exceeds `target`, it is visually truncated.
pub fn pad_right(s: &str, target: usize) -> String {
    use std::cmp::Ordering;
    let vlen = visual_len(s);
    match vlen.cmp(&target) {
        Ordering::Equal => s.to_string(),
        Ordering::Less => format!("{s}{pad}", pad = " ".repeat(target - vlen)),
        Ordering::Greater => truncate_visual(s, target),
    }
}

/// Truncate a string to at most `max_cols` terminal columns,
/// preserving ANSI escape sequences and respecting wide characters.
pub fn truncate_visual(s: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::with_capacity(s.len());
    let mut cols = 0usize;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            out.push(ch);
            if ch == 'm' {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
            out.push(ch);
        } else {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cols + w > max_cols {
                break;
            }
            cols += w;
            out.push(ch);
        }
    }
    if cols < max_cols {
        out.push_str(&" ".repeat(max_cols - cols));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_len_plain() {
        assert_eq!(visual_len("hello"), 5);
        assert_eq!(visual_len(""), 0);
    }

    #[test]
    fn visual_len_with_ansi() {
        assert_eq!(visual_len("\x1b[32mhello\x1b[0m"), 5);
        assert_eq!(visual_len("\x1b[38;2;255;0;0mX\x1b[0m"), 1);
    }

    #[test]
    fn pad_right_works() {
        assert_eq!(pad_right("hi", 5), "hi   ");
        assert_eq!(pad_right("hello", 3), "hel");
        assert_eq!(visual_len(&pad_right("hello", 3)), 3);
        let ansi = "\x1b[32mhi\x1b[0m";
        let padded = pad_right(ansi, 5);
        assert_eq!(visual_len(&padded), 5);
        assert!(padded.starts_with("\x1b[32m"));
    }
}
