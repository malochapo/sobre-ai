use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    Core,
    Internal,
    Arch,
    Debug,
    Memory,
    Metrics,
    Session,
}

impl ToolCategory {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "core" => Some(Self::Core),
            "arch" | "architecture" => Some(Self::Arch),
            "debug" | "profiling" => Some(Self::Debug),
            "memory" | "semantic" => Some(Self::Memory),
            "metrics" | "stats" => Some(Self::Metrics),
            "session" => Some(Self::Session),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Internal => "internal",
            Self::Arch => "arch",
            Self::Debug => "debug",
            Self::Memory => "memory",
            Self::Metrics => "metrics",
            Self::Session => "session",
        }
    }
}

#[allow(clippy::match_same_arms)]
pub fn categorize_tool(name: &str) -> ToolCategory {
    match name {
        // Internal: meta/self-referential + automated mechanisms (never exposed)
        "ctx_response" => ToolCategory::Internal,

        // Core: always visible. Must cover every CORE_TOOL_NAMES entry —
        // otherwise the category gate silently drops a lazy-core tool for
        // list_changed-capable clients (ctx_expand was lost this way, #575).
        // ctx_callgraph joined the lazy core in #578 (INTENT routes
        // callers/impact to it), so it must be Core here too.
        // ctx_patch joined in #1008 (anchored editing is a core workflow).
        "ctx_read" | "ctx_search" | "ctx_shell" | "shell" | "ctx_tree" | "ctx_patch"
        | "ctx_session" | "ctx_checkpoint" | "ctx_knowledge" | "ctx_graph" | "ctx_callgraph"
        | "ctx_expand" => ToolCategory::Core,

        // Merged tools (redirects in registry, treated as Core for backward compat)
        "ctx_outline" | "ctx_context" => ToolCategory::Core,

        // Arch: on-demand architecture analysis
        "ctx_impact" | "ctx_refactor" => ToolCategory::Arch,

        // Debug/Verify: on-demand quality analysis
        "ctx_review" => ToolCategory::Debug,

        // Provider + URL/Git readers + the MCP gateway are Core: gateways to
        // external context (GitHub issues, Jira, Postgres, web pages, YouTube,
        // remote git repos, downstream MCP servers) — always available.
        // ctx_semantic_search is a first-class retrieval tool (advertised in the
        // lean core, #422) — keep it Core so the default category gate never hides
        // it, the very reason agents stopped reaching for it.
        "ctx_url_read" => ToolCategory::Core,

        // Memory: on-demand artifact retrieval
        "ctx_artifacts" => ToolCategory::Memory,

        // Batch: on-demand batch/PR/sandbox tools
        "ctx_execute" => ToolCategory::Metrics,

        // Multi-agent: on-demand collaboration
        "ctx_workflow" => ToolCategory::Session,

        _ => ToolCategory::Core,
    }
}

#[derive(Debug)]
pub struct DynamicToolState {
    active_categories: HashSet<ToolCategory>,
    supports_list_changed: bool,
}

impl Default for DynamicToolState {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicToolState {
    pub fn new() -> Self {
        let mut active = HashSet::new();
        active.insert(ToolCategory::Core);
        active.insert(ToolCategory::Session);
        Self {
            active_categories: active,
            supports_list_changed: false,
        }
    }

    /// Creates state with categories from config (env var > config.toml > default).
    pub fn from_config(categories: &[String]) -> Self {
        let mut active = HashSet::new();
        active.insert(ToolCategory::Core);
        for cat_str in categories {
            if let Some(cat) = ToolCategory::parse(cat_str) {
                active.insert(cat);
            }
        }
        Self {
            active_categories: active,
            supports_list_changed: false,
        }
    }

    pub fn set_supports_list_changed(&mut self, val: bool) {
        self.supports_list_changed = val;
    }

    pub fn supports_list_changed(&self) -> bool {
        self.supports_list_changed
    }

    pub fn unload_category(&mut self, cat: ToolCategory) -> bool {
        if cat == ToolCategory::Core || cat == ToolCategory::Internal {
            return false;
        }
        self.active_categories.remove(&cat)
    }

    pub fn is_tool_active(&self, name: &str) -> bool {
        let cat = categorize_tool(name);
        if cat == ToolCategory::Internal {
            return false;
        }
        if !self.supports_list_changed {
            return true;
        }
        self.active_categories.contains(&cat)
    }
}

static GLOBAL: OnceLock<Mutex<DynamicToolState>> = OnceLock::new();

pub fn global() -> &'static Mutex<DynamicToolState> {
    GLOBAL.get_or_init(|| Mutex::new(DynamicToolState::new()))
}

/// Initializes the global state from user config (env var > config.toml > default).
/// Call once during server startup after config is loaded.
/// If the global was already initialized (e.g. by a concurrent `global()` call),
/// applies the categories to the existing state instead.
pub fn init_from_config(categories: &[String]) {
    if GLOBAL
        .set(Mutex::new(DynamicToolState::from_config(categories)))
        .is_err()
        && let Ok(mut state) = global().lock()
    {
        let desired = DynamicToolState::from_config(categories);
        *state = desired;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_tools_filtered_when_list_changed() {
        let mut state = DynamicToolState::new();
        state.set_supports_list_changed(true);
        assert!(!state.is_tool_active("ctx_review"));
        assert!(!state.is_tool_active("ctx_impact"));
        assert!(state.is_tool_active("ctx_read"));
    }

    #[test]
    fn cannot_unload_core() {
        let mut state = DynamicToolState::new();
        assert!(!state.unload_category(ToolCategory::Core));
    }

    #[test]
    fn all_tools_visible_without_list_changed() {
        let state = DynamicToolState::new();
        assert!(state.is_tool_active("ctx_graph"));
        // Without list_changed nothing registered may be hidden; this also
        // proves no live tool is declared Internal.
        for name in crate::server::registry::build_registry().names() {
            assert!(
                state.is_tool_active(name),
                "{name} hidden without list_changed"
            );
        }
    }

    // --- from_config: basic scenarios ---

    #[test]
    fn from_config_core_arch_memory() {
        let cats = vec!["core".to_string(), "arch".to_string(), "memory".to_string()];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_read"));
        assert!(state.is_tool_active("ctx_impact"));
        assert!(state.is_tool_active("ctx_artifacts"));
        assert!(!state.is_tool_active("ctx_review"));
        assert!(!state.is_tool_active("ctx_execute"));
    }

    #[test]
    fn from_config_empty_still_has_core() {
        let mut state = DynamicToolState::from_config(&[]);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_read"));
        assert!(!state.is_tool_active("ctx_impact"));
        assert!(!state.is_tool_active("ctx_review"));
        assert!(!state.is_tool_active("ctx_execute"));
        assert!(!state.is_tool_active("ctx_artifacts"));
    }

    // --- from_config: all categories ---

    #[test]
    fn from_config_all_categories_enables_everything_except_internal() {
        let cats = vec![
            "core".to_string(),
            "arch".to_string(),
            "debug".to_string(),
            "memory".to_string(),
            "metrics".to_string(),
            "session".to_string(),
        ];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        for name in crate::server::registry::build_registry().names() {
            assert!(
                state.is_tool_active(name),
                "{name} hidden with every category enabled"
            );
        }
        assert!(state.is_tool_active("ctx_workflow"));
    }

    // --- from_config: single category ---

    #[test]
    fn from_config_only_debug() {
        let cats = vec!["debug".to_string()];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_read"));
        assert!(state.is_tool_active("ctx_review"));
        assert!(!state.is_tool_active("ctx_impact"));
        assert!(!state.is_tool_active("ctx_execute"));
        assert!(!state.is_tool_active("ctx_workflow"));
    }

    // --- from_config: invalid categories are silently ignored ---

    #[test]
    fn from_config_ignores_unknown_categories() {
        let cats = vec![
            "core".to_string(),
            "nonexistent".to_string(),
            "foobar".to_string(),
        ];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_read"));
        assert!(!state.is_tool_active("ctx_impact"));
    }

    #[test]
    fn from_config_only_invalid_still_has_core() {
        let cats = vec!["invalid".to_string(), "bogus".to_string()];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_read"));
        assert!(!state.is_tool_active("ctx_review"));
    }

    // --- from_config: duplicate categories are idempotent ---

    // --- from_config: internal category is never user-activatable ---

    #[test]
    fn from_config_internal_category_not_parseable() {
        assert!(ToolCategory::parse("internal").is_none());
    }

    // --- from_config: category aliases work ---

    #[test]
    fn from_config_alias_architecture_maps_to_arch() {
        let cats = vec!["architecture".to_string()];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_impact"));
    }

    #[test]
    fn from_config_alias_profiling_maps_to_debug() {
        let cats = vec!["profiling".to_string()];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_review"));
    }

    #[test]
    fn from_config_alias_semantic_maps_to_memory() {
        let cats = vec!["semantic".to_string()];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_artifacts"));
    }

    // --- from_config: subsequent load/unload still works ---

    #[test]
    fn from_config_then_unload_non_core_category() {
        let cats = vec!["core".to_string(), "arch".to_string()];
        let mut state = DynamicToolState::from_config(&cats);
        state.set_supports_list_changed(true);
        assert!(state.is_tool_active("ctx_impact"));
        state.unload_category(ToolCategory::Arch);
        assert!(!state.is_tool_active("ctx_impact"));
    }

    // --- from_config: without list_changed, all tools visible ---

    #[test]
    fn from_config_without_list_changed_shows_all() {
        let cats = vec!["core".to_string()];
        let state = DynamicToolState::from_config(&cats);
        assert!(state.is_tool_active("ctx_impact"));
        assert!(state.is_tool_active("ctx_review"));
        for name in crate::server::registry::build_registry().names() {
            assert!(
                state.is_tool_active(name),
                "{name} hidden without list_changed"
            );
        }
    }

    #[test]
    fn categorize_known_tools() {
        assert_eq!(categorize_tool("ctx_read"), ToolCategory::Core);
        assert_eq!(categorize_tool("ctx_graph"), ToolCategory::Core);
        // #1008: anchored editing is a core workflow — an explicit Core entry so
        // the category gate can never hide the lazy-core editor.
        assert_eq!(categorize_tool("ctx_patch"), ToolCategory::Core);
        assert_eq!(categorize_tool("ctx_review"), ToolCategory::Debug);
        assert_eq!(categorize_tool("ctx_impact"), ToolCategory::Arch);
        assert_eq!(categorize_tool("ctx_refactor"), ToolCategory::Arch);
        assert_eq!(categorize_tool("ctx_execute"), ToolCategory::Metrics);
        // #422/#509: semantic search is ctx_search(action=semantic); it must stay Core.
        assert_eq!(categorize_tool("ctx_search"), ToolCategory::Core);
        assert_eq!(categorize_tool("ctx_artifacts"), ToolCategory::Memory);
        assert_eq!(categorize_tool("ctx_workflow"), ToolCategory::Session);
        // Internal means "never listed": no registered tool may land there.
        for name in crate::server::registry::build_registry().names() {
            assert_ne!(
                categorize_tool(name),
                ToolCategory::Internal,
                "{name} would never be listed"
            );
        }
    }
}
