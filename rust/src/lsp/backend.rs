//! Backend abstraction for LSP-style code intelligence: `LspClient` (a stdio
//! language server, see client.rs) and `HeadlessBackend` (symbol edits only,
//! see edit_apply.rs).

use lsp_types::{GotoDefinitionResponse, Location, Position, Uri, WorkspaceEdit};

/// A single symbol entry from a file's structure overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolOverviewItem {
    pub name: String,
    pub kind: String,
    /// 1-indexed line.
    pub line: u32,
}

/// A 0-based, half-open text range (LSP/wire convention: start inclusive, end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange0Based {
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
}

/// A resolved, ready-to-apply edit. The `name_path` → range resolution has already
/// happened in `ctx_refactor`; the backend only ever sees an absolute path + range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeEdit {
    /// Absolute, jail-checked path of the file to edit.
    pub abs_path: String,
    /// Project-relative path.
    pub rel_path: String,
    /// The canonical edit boundary (same in IDE and headless paths).
    pub range: TextRange0Based,
    /// Final text to write into `range` (indentation already baked in by Rust).
    pub text: String,
    /// Optional md5-hex of the current content of `range`; mismatch → CONFLICT.
    pub expected_hash: Option<String>,
}

/// Outcome of applying a `RangeEdit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    pub applied: bool,
    /// Range covering the newly written text after the edit.
    pub new_range: TextRange0Based,
    /// The text that now occupies `new_range`.
    pub edited_text: String,
    /// Compact human-readable diff (removed/added lines).
    pub diff: String,
}

/// Code-intelligence backend. `Send` so instances can live in the global
/// `BACKENDS` cache (`Mutex<HashMap<String, Box<dyn LspBackend>>>`).
pub trait LspBackend: Send {
    fn open_file(&mut self, uri: &Uri, language_id: &str, text: &str) -> Result<(), String>;
    fn references(
        &mut self,
        uri: &Uri,
        position: Position,
        scope: &str,
    ) -> Result<Vec<Location>, String>;
    fn definition(
        &mut self,
        uri: &Uri,
        position: Position,
    ) -> Result<GotoDefinitionResponse, String>;
    fn implementations(
        &mut self,
        uri: &Uri,
        position: Position,
        scope: &str,
    ) -> Result<Vec<Location>, String>;
    fn rename(
        &mut self,
        uri: &Uri,
        position: Position,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, String>;

    fn symbols_overview(&mut self, uri: &Uri) -> Result<Vec<SymbolOverviewItem>, String> {
        // v2a §5.2: lossless headless default via the tree-sitter symbol index
        // (same source as ctx_symbol/ctx_outline).
        let abs = crate::lsp::client::uri_to_file_path(uri)
            .ok_or_else(|| "symbols_overview: bad uri".to_string())?;
        Ok(crate::lsp::edit_apply::overview_from_index(&abs))
    }

    /// Replace a symbol's full declaration range with `edit.text`.
    /// Applied by the headless local range write.
    fn replace_symbol_body(&mut self, edit: &RangeEdit) -> Result<EditResult, String> {
        crate::lsp::edit_apply::local_range_write(edit)
    }
    /// Insert a new sibling before the anchor symbol (range is zero-width at the
    /// anchor start line; indentation already baked into `edit.text`).
    fn insert_before_symbol(&mut self, edit: &RangeEdit) -> Result<EditResult, String> {
        crate::lsp::edit_apply::local_range_write(edit)
    }
    /// Insert a new sibling after the anchor symbol (range is zero-width at the
    /// line following the anchor; indentation already baked into `edit.text`).
    fn insert_after_symbol(&mut self, edit: &RangeEdit) -> Result<EditResult, String> {
        crate::lsp::edit_apply::local_range_write(edit)
    }
}
