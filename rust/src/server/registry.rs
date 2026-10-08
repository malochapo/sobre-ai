use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::Tool;

use super::tool_trait::McpTool;

/// Central registry mapping tool names to their trait-based handlers.
/// Every tool is trait-based and resolved here; the earlier
/// match-cascade dispatch has been fully retired.
///
/// Handlers are stored behind `Arc` (not `Box`) so the dispatch layer can hand
/// an owned, `'static` handle to `tokio::task::spawn_blocking`. That lets a
/// blocking handler run on the dedicated blocking pool under a watchdog
/// deadline instead of pinning a scarce core worker via `block_in_place`
/// (#271 — a hung handler must never swallow the JSON-RPC response).
pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn McpTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn McpTool>) {
        let name = tool.name();
        self.tools.insert(name, Arc::from(tool));
    }

    /// Clone an owned, `'static` handle to a registered tool.
    ///
    /// The returned `Arc` can be moved into `spawn_blocking`, so the dispatch
    /// layer can execute the (synchronous) handler off the async core workers
    /// under a watchdog (#271).
    pub fn get_arc(&self, name: &str) -> Option<Arc<dyn McpTool>> {
        self.tools.get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Returns MCP Tool definitions for all registered tools.
    /// Used by `list_tools` to expose schemas to clients.
    /// Applies MCP `ToolAnnotations` (readOnlyHint, destructiveHint) so clients
    /// can make informed decisions about tool usage in restricted contexts.
    pub fn tool_defs(&self) -> Vec<Tool> {
        let mut defs: Vec<Tool> = self.tools.values().map(|t| t.tool_def()).collect();
        defs.sort_by(|a, b| a.name.as_ref().cmp(b.name.as_ref()));
        crate::tool_defs::apply_tool_annotations(defs)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.tools.keys().copied().collect();
        names.sort_unstable();
        names
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Number of registered MCP tools — the single source of truth for the
/// "N MCP tools" count shown in `--help`, the README, and the feature catalog.
/// Deriving it here means the count can never drift from the actual registry.
pub fn tool_count() -> usize {
    build_registry().len()
}

/// Register all trait-based tools. Called once during server startup.
/// New tools are added here as their `McpTool` implementation lands.
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    use crate::tools::registered;
    registry.register(Box::new(registered::ctx_tree::CtxTreeTool));
    registry.register(Box::new(registered::ctx_outline::CtxOutlineTool));
    registry.register(Box::new(registered::ctx_expand::CtxExpandTool));
    registry.register(Box::new(registered::ctx_refactor::CtxRefactorTool));
    registry.register(Box::new(registered::ctx_review::CtxReviewTool));
    registry.register(Box::new(registered::ctx_impact::CtxImpactTool));
    registry.register(Box::new(registered::ctx_read::CtxReadTool));
    registry.register(Box::new(registered::ctx_patch::CtxPatchTool));
    registry.register(Box::new(registered::ctx_glob::CtxGlobTool));
    registry.register(Box::new(registered::ctx_shell::CtxShellTool));
    registry.register(Box::new(registered::ctx_search::CtxSearchTool));
    registry.register(Box::new(registered::ctx_checkpoint::CtxCheckpointTool));
    registry.register(Box::new(registered::ctx_compose::CtxComposeTool));

    // Utility tools (migrated from dispatch/utility_tools.rs)
    registry.register(Box::new(registered::ctx_graph::CtxGraphTool));

    // Session tools (migrated from legacy dispatch)
    registry.register(Box::new(registered::ctx_session::CtxSessionTool));
    registry.register(Box::new(registered::ctx_knowledge::CtxKnowledgeTool));

    registry
}

/// Whether a tool name is registered, resolved against a set built once.
///
/// [`build_registry`] allocates every tool on each call, so callers that only
/// need to know whether a name exists — the injected rules ask a dozen times
/// per render — must not go through it.
pub fn is_registered(name: &str) -> bool {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<&'static str>> =
        std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| build_registry().names().into_iter().collect())
        .contains(name)
}

/// Every `ctx_*` token in `text` that does not name a registered tool.
///
/// Text sent to the agent — tool descriptions, injected rules, prompt bodies,
/// recovery footers, host hook blocks — names tools by string, and nothing
/// links a string to the registry. A tool leaves `build_registry()` and the
/// sentences recommending it stay, telling the agent to call something that
/// answers `Unknown tool`. This is the check that closes that class.
///
/// A token is `ctx_` followed by `[a-z0-9_]`, bounded on both sides by a
/// character that cannot continue an identifier, so `ctx_search(action=symbol)`
/// yields `ctx_search` and nothing else.
#[cfg(test)]
pub fn unregistered_tool_mentions(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find("ctx_") {
        let start = i + off;
        let mut end = start + 4;
        while end < bytes.len() && is_ident(bytes[end]) {
            end += 1;
        }
        let bounded_left = start == 0 || !is_ident(bytes[start - 1]);
        if bounded_left && end > start + 4 {
            let token = &text[start..end];
            if !is_registered(token) && !out.iter().any(|t| t == token) {
                out.push(token.to_string());
            }
        }
        i = end.max(start + 4);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregistered_tool_mentions_finds_only_dead_names() {
        assert_eq!(
            unregistered_tool_mentions("use ctx_search(action=symbol) or ctx_symbol"),
            vec!["ctx_symbol".to_string()],
        );
        assert!(unregistered_tool_mentions("ctx_read, ctx_patch.").is_empty());
        assert!(unregistered_tool_mentions("my_ctx_thing and ctx_").is_empty());
    }

    /// Every tool description and parameter description the agent receives
    /// names only registered tools.
    #[test]
    fn tool_descriptions_name_only_registered_tools() {
        let mut offenders: Vec<String> = Vec::new();
        for def in build_registry().tool_defs() {
            let mut text = def.description.as_deref().unwrap_or_default().to_string();
            text.push('\n');
            text.push_str(&serde_json::Value::Object((*def.input_schema).clone()).to_string());
            for dead in unregistered_tool_mentions(&text) {
                offenders.push(format!("{} -> {dead}", def.name));
            }
        }
        assert!(
            offenders.is_empty(),
            "tool definitions name tools that are not registered: {offenders:#?}"
        );
    }

    #[test]
    fn is_registered_agrees_with_a_freshly_built_registry() {
        let registry = build_registry();
        for name in registry.names() {
            assert!(is_registered(name), "{name} is registered but not reported");
        }
        assert!(!is_registered("ctx_does_not_exist_xyz"));
    }

    #[test]
    fn get_arc_returns_owned_handle_for_known_tool() {
        let registry = build_registry();
        let arc = registry.get_arc("ctx_tree");
        assert!(arc.is_some(), "ctx_tree must be registered");
        assert_eq!(arc.unwrap().name(), "ctx_tree");
    }

    #[test]
    fn get_arc_is_none_for_unknown_tool() {
        let registry = build_registry();
        assert!(registry.get_arc("ctx_does_not_exist_xyz").is_none());
    }

    #[test]
    fn get_arc_finds_a_core_tool() {
        assert!(build_registry().get_arc("ctx_read").is_some());
    }
}
