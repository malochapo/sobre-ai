//! Text from binary documents for the search index.
//!
//! Text formats (code, Markdown, JSON, CSV, HTML…) are indexed as UTF-8 as
//! they are. The one binary format the indexer reads is PDF, through
//! [`super::web::pdf`].

use std::path::Path;

/// Whether `path` is a binary document that must go through [`text`] before
/// it can be indexed. Only PDF.
#[must_use]
pub fn is_binary_document(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("pdf")
    )
}

/// The text of a PDF, or an empty string when it has none (empty,
/// malformed or image-only input). Never panics.
#[must_use]
pub fn text(bytes: &[u8]) -> String {
    super::web::pdf::extract_text(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_pdf_gives_no_text() {
        for input in [b"".as_slice(), b"not a pdf", b"%PDF-1.7 truncated"] {
            assert!(text(input).is_empty(), "{input:?}");
        }
    }

    #[test]
    fn binary_document_predicate_matches_pdf_only() {
        assert!(is_binary_document(Path::new("a.pdf")));
        assert!(is_binary_document(Path::new("A.PDF")));
        for f in ["p.html", "d.json", "t.csv", "m.eml", "n.txt", "s.rs"] {
            assert!(!is_binary_document(Path::new(f)), "{f}");
        }
    }
}
