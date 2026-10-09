//! Post-edit syntax report: a tree-sitter parse check that tells the agent
//! when an edit turned a *cleanly parsing* file into a broken one.
//!
//! The edit is always written (invariant 5: the agent decides); the report
//! says what broke. Only the *clean → broken* transition is reported: a file
//! that already had parse errors may be mid-fix, and a language without a
//! grammar gets no opinion. The decision lives in [`syntax_notice`];
//! [`check_syntax`] is the raw parse probe.

/// Outcome of a tree-sitter parse probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxCheck {
    /// `true` when the parse tree contains an ERROR or MISSING node.
    pub has_error: bool,
    /// 1-based line of the first error/missing node, when one was located.
    pub first_error_line: Option<usize>,
}

/// Parse `content` as `ext` and report whether the tree has syntax errors.
///
/// Returns `None` when the language is unsupported or tree-sitter is compiled
/// out — the caller then skips the gate rather than guessing.
#[cfg(feature = "tree-sitter")]
pub fn check_syntax(content: &str, ext: &str) -> Option<SyntaxCheck> {
    use std::cell::RefCell;
    use tree_sitter::Parser;

    let language = crate::core::deep_queries::get_language(ext)?;

    thread_local! {
        static PARSER: RefCell<Parser> = RefCell::new(Parser::new());
    }

    let tree = PARSER.with(|p| {
        let mut parser = p.borrow_mut();
        parser.set_language(&language).ok()?;
        parser.parse(content.as_bytes(), None)
    })?;

    let root = tree.root_node();
    if !root.has_error() {
        return Some(SyntaxCheck {
            has_error: false,
            first_error_line: None,
        });
    }
    Some(SyntaxCheck {
        has_error: true,
        first_error_line: first_error_line(root),
    })
}

#[cfg(not(feature = "tree-sitter"))]
pub fn check_syntax(_content: &str, _ext: &str) -> Option<SyntaxCheck> {
    None
}

/// Depth-first search for the first ERROR/MISSING node, returning its 1-based
/// start line. Only descends into subtrees that actually contain an error, so it
/// is effectively O(error-path), not O(tree).
#[cfg(feature = "tree-sitter")]
fn first_error_line(node: tree_sitter::Node) -> Option<usize> {
    if node.is_error() || node.is_missing() {
        return Some(node.start_position().row + 1);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_error() || child.is_missing() {
            return Some(child.start_position().row + 1);
        }
        if child.has_error()
            && let Some(line) = first_error_line(child)
        {
            return Some(line);
        }
    }
    None
}

/// The notice for an edit that broke a cleanly parsing file; `None` for every
/// other case: unsupported language, tree-sitter off, an already-broken
/// pre-edit file, or a post-edit file that still parses.
#[must_use]
pub fn syntax_notice(ext: &str, old_content: &str, new_content: &str) -> Option<String> {
    // Pre-edit must parse cleanly for a regression to be meaningful.
    let pre = check_syntax(old_content, ext)?;
    if pre.has_error {
        return None;
    }
    let post = check_syntax(new_content, ext)?;
    if !post.has_error {
        return None;
    }
    let loc = post
        .first_error_line
        .map_or_else(String::new, |l| format!(" near line {l}"));
    Some(format!(
        "⚠ syntax: this edit leaves a syntax error{loc} (.{ext}); the file parsed \
         cleanly before it."
    ))
}

#[cfg(all(test, feature = "tree-sitter"))]
mod tests {
    use super::*;

    #[test]
    fn clean_code_has_no_error() {
        let c = check_syntax("fn main() {}\n", "rs").unwrap();
        assert!(!c.has_error);
        assert_eq!(c.first_error_line, None);
    }

    #[test]
    fn broken_code_reports_error_with_line() {
        // Missing closing brace → parse error.
        let c = check_syntax("fn main() {\n    let x =\n", "rs").unwrap();
        assert!(c.has_error);
        assert!(c.first_error_line.is_some());
    }

    #[test]
    fn unsupported_extension_is_none() {
        assert!(check_syntax("anything at all", "unknownext").is_none());
    }

    #[test]
    fn clean_to_broken_is_reported() {
        let old = "fn main() {}\n";
        let new = "fn main() {\n"; // unbalanced brace
        let notice = syntax_notice("rs", old, new).expect("clean→broken must be reported");
        assert!(notice.contains("syntax error near line"), "{notice}");
    }

    #[test]
    fn no_notice_for_broken_to_broken() {
        // Pre-edit already broken → never our regression to block (model may fix).
        let old = "fn main() {\n"; // broken
        let new = "fn main( {\n"; // still broken
        assert!(syntax_notice("rs", old, new).is_none());
    }

    #[test]
    fn no_notice_for_clean_to_clean() {
        let old = "fn main() {}\n";
        let new = "fn main() { let x = 1; }\n";
        assert!(syntax_notice("rs", old, new).is_none());
    }

    #[test]
    fn no_notice_for_unsupported_language() {
        // No grammar → no opinion, always allow.
        assert!(syntax_notice("unknownext", "valid", "{[(").is_none());
    }

    #[test]
    fn no_notice_for_broken_being_fixed() {
        let old = "fn main() {\n"; // broken
        let new = "fn main() {}\n"; // fixed
        assert!(syntax_notice("rs", old, new).is_none());
    }
}
