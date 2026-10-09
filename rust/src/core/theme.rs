//! Terminal text helpers: the ANSI attributes the CLI uses, switched off under
//! `NO_COLOR` or when the stream is not a terminal.
//!
//! Code that formats colour into a string prints it with [`crate::outln!`] or
//! [`crate::errln!`], which drop every colour code when colour is off.

use std::borrow::Cow;
use std::io::IsTerminal;

fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal()
}

fn no_color_stderr() -> bool {
    std::env::var_os("NO_COLOR").is_some() || !std::io::stderr().is_terminal()
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

/// `text` for stdout: unchanged on a colour terminal, without its ANSI colour
/// codes otherwise.
#[must_use]
pub fn paint(text: &str) -> Cow<'_, str> {
    if no_color() {
        strip_ansi(text)
    } else {
        Cow::Borrowed(text)
    }
}

/// [`paint`] for stderr.
#[must_use]
pub fn paint_err(text: &str) -> Cow<'_, str> {
    if no_color_stderr() {
        strip_ansi(text)
    } else {
        Cow::Borrowed(text)
    }
}

/// Whether tracing output on stderr may carry colour.
#[must_use]
pub fn stderr_color() -> bool {
    !no_color_stderr()
}

/// `text` without its `ESC [ … m` colour sequences.
#[must_use]
pub fn strip_ansi(text: &str) -> Cow<'_, str> {
    if !text.contains('\x1b') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    Cow::Owned(out)
}

/// `println!` that drops colour codes under `NO_COLOR` or off a terminal.
#[macro_export]
macro_rules! outln {
    () => {
        println!()
    };
    ($($arg:tt)*) => {
        println!("{}", $crate::core::theme::paint(&format!($($arg)*)))
    };
}

/// `eprintln!` that drops colour codes under `NO_COLOR` or off a terminal.
#[macro_export]
macro_rules! errln {
    () => {
        eprintln!()
    };
    ($($arg:tt)*) => {
        eprintln!("{}", $crate::core::theme::paint_err(&format!($($arg)*)))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_colour_sequences_only() {
        assert_eq!(strip_ansi("\x1b[32m✓\x1b[0m ok"), "✓ ok");
        assert_eq!(strip_ansi("\x1b[1m\x1b[97mtitle\x1b[0m"), "title");
        assert_eq!(strip_ansi("plain [text]"), "plain [text]");
    }
}
