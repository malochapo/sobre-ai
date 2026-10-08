use rmcp::ErrorData;
use rmcp::model::{ContentBlock, Tool};
use serde_json::{Map, Value};

/// Outcome of a call, carried alongside the rendered text so the MCP dispatch
/// layer can surface failures in protocol metadata (`isError`) instead of only
/// in the text (GitHub #389: clients had no programmatic way to detect
/// failures and resorted to fragile regex matching).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellOutcome {
    /// The command ran; carries its real exit code (0 = success).
    Exit(i32),
    /// A non-shell tool failed: an error, a conflict or an unknown action.
    Failed,
}

impl ShellOutcome {
    /// Whether this outcome must be reported as a tool error (`isError: true`).
    pub fn is_error(self) -> bool {
        match self {
            ShellOutcome::Exit(code) => code != 0,
            ShellOutcome::Failed => true,
        }
    }

    /// Structured payload for `CallToolResult.structuredContent`, so guards
    /// can read `exitCode` instead of parsing output text. Success
    /// (exit 0) intentionally returns `None` — the happy path stays
    /// token-neutral for clients that render structured content.
    pub fn structured(self) -> Option<serde_json::Value> {
        match self {
            ShellOutcome::Exit(0) | ShellOutcome::Failed => None,
            ShellOutcome::Exit(code) => Some(serde_json::json!({ "exitCode": code })),
        }
    }
}

/// Result returned by an McpTool handler.
pub struct ToolOutput {
    pub text: String,
    pub original_tokens: usize,
    pub saved_tokens: usize,
    pub mode: Option<String>,
    /// Path associated with the tool call (for record_call_with_path).
    pub path: Option<String>,
    /// Set by shell-executing tools so dispatch can populate `isError` +
    /// `structuredContent` on the MCP result (GitHub #389). `None` for tools
    /// that don't run shell commands.
    pub shell_outcome: Option<ShellOutcome>,
    /// Override content blocks for non-text responses (e.g. images).
    /// When set, dispatch uses these instead of wrapping `text` in TextContent.
    pub content_blocks: Option<Vec<ContentBlock>>,
}

impl ToolOutput {
    pub fn simple(text: String) -> Self {
        Self {
            text,
            original_tokens: 0,
            saved_tokens: 0,
            mode: None,
            path: None,
            shell_outcome: None,
            content_blocks: None,
        }
    }

    /// Construct a ToolOutput for image/binary content blocks.
    /// Bypasses all text processing in the dispatch pipeline.
    pub fn image(blocks: Vec<ContentBlock>, path: String) -> Self {
        Self {
            text: String::new(),
            original_tokens: 0,
            saved_tokens: 0,
            mode: Some("image".to_string()),
            path: Some(path),
            shell_outcome: None,
            content_blocks: Some(blocks),
        }
    }
}

/// Trait for a self-contained MCP tool. Each tool provides its own schema
/// definition and handler, eliminating the possibility of schema/handler drift.
///
/// This trait is the plugin interface for LcpTools: any implementation can be
/// registered at runtime via `ToolRegistry::register()`. Future plugin system
/// will load implementations from shared libraries or subprocess bridges.
///
/// Handlers are synchronous because all existing tool handlers are sync.
/// The async boundary (cache locks, session reads) is handled by the dispatch
/// layer before calling `handle`.
pub trait McpTool: Send + Sync {
    /// Tool name as registered in the MCP protocol (e.g. "ctx_tree").
    fn name(&self) -> &'static str;

    /// MCP tool definition including JSON schema.
    fn tool_def(&self) -> Tool;

    /// Execute the tool. Args are the raw JSON-RPC arguments.
    /// `ctx` provides access to resolved paths and project state.
    fn handle(&self, args: &Map<String, Value>, ctx: &ToolContext)
    -> Result<ToolOutput, ErrorData>;

    /// Whether *this* invocation yields a machine-readable payload (e.g. JSON)
    /// that must reach the client byte-exact and parseable. When `true`, the
    /// dispatch pipeline suppresses every human-oriented decoration
    /// (auto-context briefing, verify footer, hints, checkpoints) and terse
    /// compression for this call, returning the pure body.
    /// Keyed on `args` because most tools emit machine-readable output only for
    /// an opt-in format flag (e.g. `format=json`); defaults to `false` (#990).
    fn produces_machine_readable(&self, _args: Option<&Map<String, Value>>) -> bool {
        false
    }

    /// Whether this tool's text starts with `ERROR`, `CONFLICT` or `Unknown
    /// action` exactly when the call failed, so dispatch can mark the result
    /// `isError` ([`reports_failure`]). Tools that return file or command
    /// content keep `false`: that content may start with anything.
    fn text_reports_failures(&self) -> bool {
        false
    }
}

/// Whether a tool's text answer reports a failure: its first line, or the
/// first line of a `[file]` section in a multi-file answer, starts with one of
/// the prefixes the tool cores use for errors, conflicts and unknown actions.
pub fn reports_failure(text: &str) -> bool {
    const PREFIXES: [&str; 5] = ["ERROR", "Error:", "ERR:", "CONFLICT", "Unknown action"];
    let fails = |line: &str| PREFIXES.iter().any(|p| line.trim_start().starts_with(p));
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return false;
    };
    if fails(first) {
        return true;
    }
    let mut after_label = is_section_label(first);
    for line in lines {
        if after_label && fails(line) {
            return true;
        }
        after_label = is_section_label(line);
    }
    false
}

/// A `[path]` line that opens one file's part of a multi-file answer.
fn is_section_label(line: &str) -> bool {
    let t = line.trim();
    t.len() > 2 && t.starts_with('[') && t.ends_with(']') && !t.contains(' ')
}

/// Context passed to tool handlers. Contains pre-resolved values that
/// many tools need, avoiding repeated async lock acquisition inside
/// handlers. Extended with shared server state for tools that need
/// cache/session access.
///
/// `Clone` exists for per-op delegation (#1088): a batch op with its own
/// `path` clones the ctx and swaps in that op's resolved path.
#[derive(Clone)]
pub struct ToolContext {
    pub project_root: String,
    /// The session's shell cwd (where `ctx_shell` last ran), snapshotted like
    /// `extra_roots`: relative paths fall back to it, and a cwd in another git
    /// checkout outranks `project_root` (#707).
    pub shell_cwd: Option<String>,
    /// Session-scoped extra roots (MCP `roots/list`, config `search_roots`),
    /// snapshotted from the session so sync handlers can honor them without an
    /// async lock. Empty = single root (#403).
    pub extra_roots: Vec<String>,
    /// Pre-resolved paths keyed by argument name (e.g. "path" -> "/abs/dir").
    pub resolved_paths: std::collections::HashMap<String, String>,
    /// CRP mode for compression-aware tools.
    pub crp_mode: crate::tools::CrpMode,
    /// Shared cache handle for tools that need read/write access.
    pub cache: Option<crate::tools::SharedCache>,
    /// Shared session handle for tools that need session access.
    pub session: Option<std::sync::Arc<tokio::sync::RwLock<crate::core::session::SessionState>>>,
    /// Errors from path resolution (PathJail rejection, secret path, etc.).
    /// Keyed by argument name (e.g. "path" -> "path escapes project root: ...").
    pub path_errors: std::collections::HashMap<String, String>,
    /// Shared in-memory BM25 index cache for semantic search.
    pub bm25_cache: Option<crate::core::bm25_cache::SharedBm25Cache>,
    /// Progress notifications for this request, when the client sent a token.
    pub progress_sender: Option<crate::server::progress::ProgressSender>,
}

impl Default for ToolContext {
    /// Minimal context: `project_root` empty, all shared-state handles `None`.
    /// Single source for the one-shot CLI ctx (`cli::call_cmd::oneshot_ctx`) and
    /// the `empty_ctx` test helper — adding a field no longer breaks either.
    fn default() -> Self {
        Self {
            project_root: String::new(),
            shell_cwd: None,
            extra_roots: Vec::new(),
            resolved_paths: std::collections::HashMap::new(),
            crp_mode: crate::tools::CrpMode::Off,
            cache: None,
            session: None,
            path_errors: std::collections::HashMap::new(),
            bm25_cache: None,
            progress_sender: None,
        }
    }
}

impl ToolContext {
    pub fn resolved_path(&self, arg: &str) -> Option<&str> {
        self.resolved_paths.get(arg).map(String::as_str)
    }

    /// Returns the path resolution error for a given key, if any.
    pub fn path_error(&self, key: &str) -> Option<&str> {
        self.path_errors.get(key).map(String::as_str)
    }

    /// Sync path resolution with the same rules as the dispatcher: a thin
    /// wrapper over [`crate::core::path_resolve::resolve_tool_path`] with
    /// `project_root` and `shell_cwd`.
    pub fn resolve_path_sync(&self, path: &str) -> Result<String, String> {
        let root = (!self.project_root.is_empty()).then_some(self.project_root.as_str());
        crate::core::path_resolve::resolve_tool_path(root, self.shell_cwd.as_deref(), path)
    }
}

// ── Arg extraction helpers (mirror server/helpers.rs for standalone use) ──

/// Extract a resolved path from context with differentiated error messages.
/// Returns descriptive errors for: missing param, PathJail rejection, wrong type.
pub fn require_resolved_path(
    ctx: &ToolContext,
    args: &Map<String, Value>,
    key: &str,
) -> Result<String, ErrorData> {
    if let Some(path) = ctx.resolved_path(key) {
        return Ok(path.to_string());
    }
    if let Some(err) = ctx.path_error(key) {
        return Err(ErrorData::invalid_params(format!("{key}: {err}"), None));
    }
    if let Some(val) = args.get(key)
        && !val.is_string()
    {
        let type_name = match val {
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Null => "null",
            Value::String(_) => unreachable!(),
        };
        return Err(ErrorData::invalid_params(
            format!("{key} must be a string, got {type_name}"),
            None,
        ));
    }
    Err(ErrorData::invalid_params(
        format!("{key} is required"),
        None,
    ))
}

pub fn get_str(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}

pub fn get_int(args: &Map<String, Value>, key: &str) -> Option<i64> {
    args.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
}

/// Read a non-negative integer argument as `usize`.
///
/// Returns `None` for missing or negative values. This avoids the
/// `negative_i64 as usize` wrap to `usize::MAX`, which previously let an agent
/// trigger unbounded allocations (e.g. `top_k`, `limit`, `max_results`) → OOM.
/// Callers should still apply a sensible upper cap on the result.
pub fn get_usize(args: &Map<String, Value>, key: &str) -> Option<usize> {
    get_int(args, key).and_then(|n| usize::try_from(n).ok())
}

pub fn get_bool(args: &Map<String, Value>, key: &str) -> Option<bool> {
    args.get(key).and_then(serde_json::Value::as_bool)
}

pub fn get_f64(args: &Map<String, Value>, key: &str) -> Option<f64> {
    args.get(key).and_then(serde_json::Value::as_f64)
}

pub fn get_str_array(args: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn empty_ctx() -> ToolContext {
        ToolContext::default()
    }

    #[test]
    fn failures_are_recognised_by_their_prefix() {
        for text in [
            "ERROR: path is required for action=impact",
            "Error: value is required for remember",
            "ERR: empty index. Try action=reindex first.",
            "Unknown action: x. Use: status, load",
            "[src/a.rs]\nCONFLICT: 1 stale anchor(s) in a.rs",
            "[a.rs]\n✓ a.rs: 1 edit\n\n[b.rs]\nERROR: file changed since read",
        ] {
            assert!(reports_failure(text), "{text}");
        }
        for text in [
            "Impact of changing src/a.rs: 2 affected files",
            "[a.rs]\n✓ a.rs: 1 edit",
            "No files depend on src/a.rs (leaf node in the dependency graph).",
            "line one\nERROR: inside the content, not a section start",
            "",
        ] {
            assert!(!reports_failure(text), "{text}");
        }
        assert!(ShellOutcome::Failed.is_error());
        assert_eq!(ShellOutcome::Failed.structured(), None);
    }

    #[test]
    fn require_resolved_path_returns_resolved() {
        let mut ctx = empty_ctx();
        ctx.resolved_paths
            .insert("path".to_string(), "/abs/file.rs".to_string());
        let args: Map<String, Value> = Map::new();
        let result = require_resolved_path(&ctx, &args, "path");
        assert_eq!(result.unwrap(), "/abs/file.rs");
    }

    #[test]
    fn require_resolved_path_surfaces_resolution_error() {
        let mut ctx = empty_ctx();
        ctx.path_errors
            .insert("path".to_string(), "path contains a null byte".to_string());
        let args: Map<String, Value> = Map::new();
        let result = require_resolved_path(&ctx, &args, "path");
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("null byte"), "got: {msg}");
    }

    #[test]
    fn require_resolved_path_detects_non_string() {
        let ctx = empty_ctx();
        let mut args: Map<String, Value> = Map::new();
        args.insert("path".to_string(), json!(42));
        let result = require_resolved_path(&ctx, &args, "path");
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("must be a string, got number"), "got: {msg}");
    }

    #[test]
    fn require_resolved_path_missing_param() {
        let ctx = empty_ctx();
        let args: Map<String, Value> = Map::new();
        let result = require_resolved_path(&ctx, &args, "path");
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("path is required"), "got: {msg}");
    }

    #[test]
    fn get_int_coerces_string_to_number() {
        let mut args = Map::new();
        args.insert("n".into(), json!("42"));
        assert_eq!(super::get_int(&args, "n"), Some(42));
    }

    #[test]
    fn get_int_native_number_still_works() {
        let mut args = Map::new();
        args.insert("n".into(), json!(7));
        assert_eq!(super::get_int(&args, "n"), Some(7));
    }

    #[test]
    fn get_int_invalid_string_returns_none() {
        let mut args = Map::new();
        args.insert("n".into(), json!("not_a_number"));
        assert_eq!(super::get_int(&args, "n"), None);
    }

    #[test]
    fn get_usize_coerces_string() {
        let mut args = Map::new();
        args.insert("n".into(), json!("100"));
        assert_eq!(super::get_usize(&args, "n"), Some(100));
    }

    #[test]
    fn get_usize_negative_string_returns_none() {
        let mut args = Map::new();
        args.insert("n".into(), json!("-5"));
        assert_eq!(super::get_usize(&args, "n"), None);
    }
}
