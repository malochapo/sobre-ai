//! Shared headless apply path for symbol-body edits (spec v2a §5.1).
//!
//! `local_range_write` is the Trait-default for `replace_symbol_body` /
//! `insert_before_symbol` / `insert_after_symbol`: it writes a resolved range
//! to disk atomically, so edits work without any running language server.

use crate::lsp::backend::{EditResult, RangeEdit, TextRange0Based};

/// Convert a 0-based (line, character) coordinate to a byte offset into `content`.
/// `line`/`character` count UTF-8 *bytes* per line (wire convention here is byte
/// columns, matching how Rust slices `&str`). Out-of-range → `Err`.
pub fn offset_of(content: &str, line: u32, character: u32) -> Result<usize, String> {
    let mut offset = 0usize;
    let mut cur_line = 0u32;
    for l in content.split_inclusive('\n') {
        if cur_line == line {
            let line_len = l.trim_end_matches('\n').len();
            if character as usize > line_len {
                return Err(format!(
                    "POSITION_OUT_OF_RANGE: character {character} past end of line {line}"
                ));
            }
            return Ok(offset + character as usize);
        }
        offset += l.len();
        cur_line += 1;
    }
    // Allow the position one past the last line (line == cur_line, character 0):
    if line == cur_line && character == 0 {
        return Ok(offset);
    }
    Err(format!(
        "POSITION_OUT_OF_RANGE: line {line} past end of file"
    ))
}

/// Apply a resolved `RangeEdit` to disk (headless). Under the file's edit
/// lock: reads the file, optionally verifies `expected_hash` against the
/// *current* bytes covered by `range` (mismatch → `CONFLICT`), replaces the
/// range with `text`, and writes through the edit tools' boundary (symlink
/// refusal, preimage guard, file mode kept). Returns the post-edit range and a
/// compact diff.
pub fn local_range_write(edit: &RangeEdit) -> Result<EditResult, String> {
    let path = std::path::Path::new(&edit.abs_path);
    let file_lock = crate::core::path_locks::per_file_lock(&edit.abs_path);
    let _guard = file_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cap = crate::core::limits::max_read_bytes();
    let pre = crate::tools::edit_io::read_preimage(path, cap, false).map_err(strip_error)?;
    let content = pre.text.as_str();

    let start = offset_of(content, edit.range.start_line, edit.range.start_char)?;
    let end = offset_of(content, edit.range.end_line, edit.range.end_char)?;
    if end < start {
        return Err("POSITION_OUT_OF_RANGE: end before start".to_string());
    }
    let old = &content[start..end];

    if let Some(expected) = edit.expected_hash.as_deref() {
        let actual = crate::core::hasher::hash_hex(old.as_bytes());
        if expected != actual {
            return Err(format!(
                "CONFLICT: range hash mismatch (expected={expected}, actual={actual})"
            ));
        }
    }

    let mut new_content = String::with_capacity(content.len() - old.len() + edit.text.len());
    new_content.push_str(&content[..start]);
    new_content.push_str(&edit.text);
    new_content.push_str(&content[end..]);

    crate::tools::edit_io::ensure_preimage_still_matches(path, &pre.fp, cap)
        .map_err(strip_error)?;
    crate::tools::edit_io::write_atomic_bytes_with_permissions(
        path,
        new_content.as_bytes(),
        Some(&pre.permissions),
    )
    .map_err(strip_error)?;
    if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
        bt.record_edit(&edit.abs_path);
    }

    let new_range = range_after_write(&content[..start], &edit.text);
    Ok(EditResult {
        applied: true,
        new_range,
        edited_text: edit.text.clone(),
        diff: build_range_diff(&edit.rel_path, old, &edit.text),
    })
}

/// Compute the 0-based range the freshly written `text` now occupies, given the
/// `prefix` (everything before the insertion point).
fn range_after_write(prefix: &str, text: &str) -> TextRange0Based {
    let (sl, sc) = line_col_at_end(prefix);
    let (dl, dc) = line_col_at_end(text);
    let end_line = sl + dl;
    let end_char = if dl == 0 { sc + dc } else { dc };
    TextRange0Based {
        start_line: sl,
        start_char: sc,
        end_line,
        end_char,
    }
}

/// (line, character) of the position *after* the last byte of `s` (0-based).
fn line_col_at_end(s: &str) -> (u32, u32) {
    let line = s.matches('\n').count() as u32;
    let col = match s.rfind('\n') {
        Some(i) => (s.len() - i - 1) as u32,
        None => s.len() as u32,
    };
    (line, col)
}

fn build_range_diff(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("--- {path}\n");
    for l in old.lines() {
        out.push_str(&format!("- {l}\n"));
    }
    for l in new.lines() {
        out.push_str(&format!("+ {l}\n"));
    }
    out
}

/// The edit boundary's errors carry an `ERROR: ` prefix; callers add their own.
fn strip_error(e: String) -> String {
    match e.strip_prefix("ERROR: ") {
        Some(rest) => rest.to_string(),
        None => e,
    }
}

/// Zero-dependency backend that carries only the Trait default-apply for the
/// three edit methods. The read methods need a language server and are
/// unsupported here (edits never call them).
pub struct HeadlessBackend;

impl crate::lsp::backend::LspBackend for HeadlessBackend {
    fn open_file(&mut self, _u: &lsp_types::Uri, _l: &str, _t: &str) -> Result<(), String> {
        Ok(())
    }
    fn references(
        &mut self,
        _u: &lsp_types::Uri,
        _p: lsp_types::Position,
    ) -> Result<Vec<lsp_types::Location>, String> {
        Err("references requires a backend".into())
    }
    fn definition(
        &mut self,
        _u: &lsp_types::Uri,
        _p: lsp_types::Position,
    ) -> Result<lsp_types::GotoDefinitionResponse, String> {
        Err("definition requires a backend".into())
    }
    fn implementations(
        &mut self,
        _u: &lsp_types::Uri,
        _p: lsp_types::Position,
    ) -> Result<Vec<lsp_types::Location>, String> {
        Err("implementations requires a backend".into())
    }
    fn rename(
        &mut self,
        _u: &lsp_types::Uri,
        _p: lsp_types::Position,
        _n: &str,
    ) -> Result<Option<lsp_types::WorkspaceEdit>, String> {
        Err("rename requires a backend".into())
    }
    // replace_symbol_body / insert_before_symbol / insert_after_symbol inherit
    // the Trait default → local_range_write.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_of_maps_lines_and_columns() {
        let s = "ab\ncde\nf";
        assert_eq!(offset_of(s, 0, 0).unwrap(), 0);
        assert_eq!(offset_of(s, 0, 2).unwrap(), 2); // end of "ab"
        assert_eq!(offset_of(s, 1, 0).unwrap(), 3); // start of "cde"
        assert_eq!(offset_of(s, 1, 3).unwrap(), 6); // end of "cde"
        assert_eq!(offset_of(s, 2, 1).unwrap(), 8); // end of "f"
    }

    #[test]
    fn offset_of_one_past_last_line_is_eof() {
        let s = "ab\ncde\n";
        assert_eq!(offset_of(s, 2, 0).unwrap(), s.len());
    }

    #[test]
    fn offset_of_rejects_overrun() {
        let s = "ab\ncde";
        assert!(offset_of(s, 0, 5).is_err());
        assert!(offset_of(s, 9, 0).is_err());
    }

    fn tmp_file(content: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Foo.txt");
        std::fs::write(&path, content).unwrap();
        (dir, path.to_string_lossy().to_string())
    }

    fn edit(abs: &str, r: TextRange0Based, text: &str, hash: Option<String>) -> RangeEdit {
        RangeEdit {
            abs_path: abs.to_string(),
            rel_path: "Foo.txt".to_string(),
            range: r,
            text: text.to_string(),
            expected_hash: hash,
        }
    }

    #[test]
    fn local_range_write_replaces_range() {
        let (_d, p) = tmp_file("aaa\nBODY\nccc\n");
        let r = TextRange0Based {
            start_line: 1,
            start_char: 0,
            end_line: 1,
            end_char: 4,
        };
        let res = local_range_write(&edit(&p, r, "NEW", None)).unwrap();
        assert!(res.applied);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "aaa\nNEW\nccc\n");
        assert_eq!(res.edited_text, "NEW");
    }

    #[test]
    fn local_range_write_zero_width_insert() {
        let (_d, p) = tmp_file("aaa\nccc\n");
        let r = TextRange0Based {
            start_line: 1,
            start_char: 0,
            end_line: 1,
            end_char: 0,
        };
        local_range_write(&edit(&p, r, "bbb\n", None)).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "aaa\nbbb\nccc\n");
    }

    #[test]
    fn local_range_write_hash_match_and_mismatch() {
        let (_d, p) = tmp_file("aaa\nBODY\nccc\n");
        let r = TextRange0Based {
            start_line: 1,
            start_char: 0,
            end_line: 1,
            end_char: 4,
        };
        let good = crate::core::hasher::hash_hex(b"BODY");
        // good hash matches current "BODY" → applies, line stays 4 chars wide ("XXXX")
        local_range_write(&edit(&p, r, "XXXX", Some(good))).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "aaa\nXXXX\nccc\n");
        // second write with a stale hash on the still-valid range → CONFLICT, file unchanged
        let err = local_range_write(&edit(&p, r, "YYYY", Some("deadbeef".into()))).unwrap_err();
        assert!(err.starts_with("CONFLICT"), "got: {err}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "aaa\nXXXX\nccc\n");
    }
}
