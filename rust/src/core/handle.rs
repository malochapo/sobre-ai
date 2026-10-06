//! Stable cross-turn symbol handles: `path#name@Lline`.
//!
//! A handle is a compact, copy-pasteable identifier for a symbol that an agent
//! can reuse across turns without re-discovering it: the project-relative file
//! path, the (possibly qualified) symbol name, and the 1-based start line, e.g.
//! `src/lib.rs#Config::load@L22`.
//!
//! Two delimiters carry the structure:
//! * `#` separates `path` from `name` — file paths never contain `#`, so the
//!   first `#` is an unambiguous split point even for trait-impl names that
//!   embed `::` (`src/x.rs#std::fmt::Display::fmt@L9`).
//! * `@L<digits>` is an *optional* line suffix, parsed only when it is a real
//!   `@L<number>` tail, so the rare symbol name containing `@` still round-trips.
//!
//! The line is a hint, not an identity: [`crate::core::graph_provider`] resolves
//! a handle by `(path, name)` first and treats `@Lline` as a tiebreak, so a
//! handle keeps resolving after the symbol drifts to a new line — strictly more
//! robust than a brittle line-only reference.
//!
//! Determinism (#498): emitting a handle is a pure function of `(path, name,
//! line)`, so any output carrying handles stays byte-stable across identical
//! re-reads and provider prompt caching still applies.

/// A parsed symbol handle. `line` is `None` when the source string omitted the
/// `@LN` suffix; resolution then falls back to `(path, name)` only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHandle {
    /// Project-relative file path (the index key prefix), e.g. `src/lib.rs`.
    pub path: String,
    /// Symbol name, possibly qualified with `::` (`Config::load`).
    pub name: String,
    /// 1-based start line, when known. Only ever a resolution tiebreak.
    pub line: Option<usize>,
}

impl SymbolHandle {
    /// Build a handle from its parts (line known).
    #[must_use]
    pub fn new(path: impl Into<String>, name: impl Into<String>, line: usize) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            line: Some(line),
        }
    }

    /// Render `path#name@Lline` (drops the `@Lline` suffix when the line is
    /// unknown). Inverse of [`SymbolHandle::parse`].
    #[must_use]
    pub fn emit(&self) -> String {
        match self.line {
            Some(line) => format!("{}#{}@L{}", self.path, self.name, line),
            None => format!("{}#{}", self.path, self.name),
        }
    }

    /// Parse `path#name[@Lline]`. Returns `None` when the `path#name` core is
    /// missing (no `#`, or an empty path/name). The `@Lline` suffix is consumed
    /// only when it is a genuine `@L<digits>` tail.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (path, rest) = s.split_once('#')?;
        if path.is_empty() || rest.is_empty() {
            return None;
        }
        if let Some(at) = rest.rfind('@') {
            let after = &rest[at + 1..];
            if let Some(digits) = after.strip_prefix('L')
                && !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
            {
                let name = &rest[..at];
                if !name.is_empty() {
                    return Some(Self {
                        path: path.to_string(),
                        name: name.to_string(),
                        line: digits.parse().ok(),
                    });
                }
            }
        }
        Some(Self {
            path: path.to_string(),
            name: rest.to_string(),
            line: None,
        })
    }
}

/// Emit a handle string from parts without constructing a [`SymbolHandle`].
/// The hot path for renderers that already hold `(path, name, line)`.
#[must_use]
pub fn emit(path: &str, name: &str, line: usize) -> String {
    SymbolHandle::new(path, name, line).emit()
}

/// One-line, self-describing usage hint (GL#580) for outputs that list located
/// symbols (outline, signatures/map, call-graph). Rather than repeat a full
/// handle on every line — the file/name/line are already shown — these outputs
/// carry this single hint telling the agent that each `name @Lstart` is
/// addressable as a stable handle. Matches the codebase's `↳ …` hint style and
/// is a constant, so it stays deterministic (#498).
pub const USAGE_HINT: &str =
    "↳ re-target any symbol: ctx_search(action=\"symbol\", handle=\"path#name@Lstart\")";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_emit_parse() {
        let h = SymbolHandle::new("src/lib.rs", "Config::load", 22);
        let s = h.emit();
        assert_eq!(s, "src/lib.rs#Config::load@L22");
        assert_eq!(SymbolHandle::parse(&s), Some(h));
    }

    #[test]
    fn emit_helper_matches_struct() {
        assert_eq!(
            emit("a/b.rs", "foo", 7),
            SymbolHandle::new("a/b.rs", "foo", 7).emit()
        );
    }

    #[test]
    fn parses_without_line_suffix() {
        let h = SymbolHandle::parse("src/lib.rs#Config::load").unwrap();
        assert_eq!(h.path, "src/lib.rs");
        assert_eq!(h.name, "Config::load");
        assert_eq!(h.line, None);
    }

    #[test]
    fn keeps_qualified_names_with_colons() {
        let h = SymbolHandle::parse("src/x.rs#std::fmt::Display::fmt@L9").unwrap();
        assert_eq!(h.path, "src/x.rs");
        assert_eq!(h.name, "std::fmt::Display::fmt");
        assert_eq!(h.line, Some(9));
    }

    #[test]
    fn name_with_at_but_no_line_is_preserved() {
        // `@foo` is not an `@L<digits>` tail, so it stays part of the name.
        let h = SymbolHandle::parse("src/x.rs#weird@name").unwrap();
        assert_eq!(h.name, "weird@name");
        assert_eq!(h.line, None);
    }

    #[test]
    fn rejects_missing_hash_or_empty_parts() {
        assert_eq!(SymbolHandle::parse("src/lib.rs"), None);
        assert_eq!(SymbolHandle::parse("#name@L1"), None);
        assert_eq!(SymbolHandle::parse("src/lib.rs#"), None);
        assert_eq!(SymbolHandle::parse(""), None);
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        let h = SymbolHandle::parse("  src/a.rs#foo@L3  ").unwrap();
        assert_eq!(h, SymbolHandle::new("src/a.rs", "foo", 3));
    }

    #[test]
    fn line_drift_still_parses_same_identity() {
        // The same (path, name) with a different line is still a valid handle —
        // the resolver, not the parser, decides identity.
        let a = SymbolHandle::parse("src/a.rs#foo@L10").unwrap();
        let b = SymbolHandle::parse("src/a.rs#foo@L999").unwrap();
        assert_eq!((a.path, a.name), (b.path, b.name));
    }
}
