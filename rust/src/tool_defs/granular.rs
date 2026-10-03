use rmcp::model::Tool;

/// Full tool definitions for the MCP `tools/list` fallback and the lazy core
/// set.
///
/// Single source of truth: derived directly from the canonical tool registry
/// (`server::registry::build_registry`) so this list can never drift from the
/// trait-based `McpTool::tool_def()` schemas. `build_registry()` is a pure,
/// cheap-to-build function (registers unit structs only), so calling it here is
/// inexpensive and keeps schemas in lock-step (#141).
pub fn granular_tool_defs() -> Vec<Tool> {
    crate::server::registry::build_registry().tool_defs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #141: the fallback / lazy tool list must stay in lock-step with the
    /// canonical registry so schemas can never drift. `granular_tool_defs`
    /// delegates to the registry; this guards against anyone reintroducing a
    /// hand-maintained copy with divergent schemas (the original bug: ctx_read
    /// default mode `full` in granular vs `auto` in the registry).
    #[test]
    fn granular_defs_match_registry() {
        let granular = granular_tool_defs();
        let registry = crate::server::registry::build_registry().tool_defs();
        assert_eq!(
            granular.len(),
            registry.len(),
            "granular must mirror the registry tool set"
        );
        for (g, r) in granular.iter().zip(registry.iter()) {
            assert_eq!(g.name, r.name, "tool name drift");
            assert_eq!(
                g.description, r.description,
                "description drift for {}",
                g.name
            );
            assert_eq!(
                g.input_schema, r.input_schema,
                "schema drift for {}",
                g.name
            );
        }
    }

    /// Read-only tools must carry `readOnlyHint: true` so MCP clients can
    /// allow them in restricted/readonly subagent contexts.
    #[test]
    fn readonly_tools_have_annotation() {
        let defs = crate::server::registry::build_registry().tool_defs();
        for t in &defs {
            let name = t.name.as_ref();
            if crate::tool_defs::READONLY_TOOL_NAMES.contains(&name) {
                let ann = t
                    .annotations
                    .as_ref()
                    .unwrap_or_else(|| panic!("READONLY tool '{name}' is missing annotations"));
                assert_eq!(
                    ann.read_only_hint,
                    Some(true),
                    "READONLY tool '{name}' must have readOnlyHint=true"
                );
            }
        }
    }

    /// Destructive tools must carry `destructiveHint: true`.
    #[test]
    fn destructive_tools_have_annotation() {
        let defs = crate::server::registry::build_registry().tool_defs();
        for t in &defs {
            let name = t.name.as_ref();
            if crate::tool_defs::DESTRUCTIVE_TOOL_NAMES.contains(&name) {
                let ann = t
                    .annotations
                    .as_ref()
                    .unwrap_or_else(|| panic!("DESTRUCTIVE tool '{name}' is missing annotations"));
                assert_eq!(
                    ann.destructive_hint,
                    Some(true),
                    "DESTRUCTIVE tool '{name}' must have destructiveHint=true"
                );
            }
        }
    }
}
