//! Server bridge for Context Kernel list-tools schema optimization.

use std::borrow::Cow;

use crate::core::context_kernel::list_tools_opt;

/// Optimizes tool descriptions when schema optimization supports the client.
#[must_use]
pub fn optimize_tools(tools: Vec<rmcp::model::Tool>, client_name: &str) -> Vec<rmcp::model::Tool> {
    if !list_tools_opt::should_optimize_for_client(client_name) {
        return tools;
    }

    let tuples = tools
        .iter()
        .map(|tool| {
            let parameter_count = tool
                .input_schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .map_or(0, serde_json::Map::len);
            (
                tool.name.to_string(),
                tool.description.as_deref().unwrap_or("").to_owned(),
                parameter_count,
            )
        })
        .collect();
    let optimized = list_tools_opt::optimize_descriptions(tuples, client_name);

    // Re-index by name, never by position. `optimize_schemas` sorts entries by
    // estimated token size and drops them when the budget is exceeded, so the
    // returned vec is neither in the input order nor necessarily the same
    // length. Zipping it against the original list handed each tool the
    // description of whichever tool happened to land at its index, and silently
    // truncated the surface when entries had been dropped.
    //
    // A tool missing from the result keeps its own description: the optimizer
    // dropping it is a budget decision about prose, not permission to stop
    // advertising it.
    let mut by_name: std::collections::HashMap<String, String> = optimized
        .into_iter()
        .map(|(name, description, _)| (name, description))
        .collect();

    tools
        .into_iter()
        .map(|mut tool| {
            if let Some(description) = by_name.remove(tool.name.as_ref()) {
                tool.description = Some(Cow::Owned(description));
            }
            tool
        })
        .collect()
}

/// Returns cumulative list-tools schema optimization statistics.
#[must_use]
pub fn summary() -> list_tools_opt::SchemaOptSummary {
    list_tools_opt::schema_opt_summary()
}

#[cfg(test)]
mod tests {
    use std::sync::MutexGuard;

    use super::list_tools_opt;
    use crate::core::context_kernel::kernel_config::{self, KERNEL_TEST_LOCK, KernelFeatures};

    fn setup() -> MutexGuard<'static, ()> {
        let guard = KERNEL_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        kernel_config::reset_features();
        list_tools_opt::reset();
        guard
    }

    #[test]
    fn disabled_returns_same_count() {
        let _guard = setup();
        let features = KernelFeatures {
            enabled: false,
            ..KernelFeatures::default()
        };
        kernel_config::update_features(features);
        let tools = vec![("tool".to_owned(), "description".to_owned(), 1)];

        assert_eq!(
            list_tools_opt::optimize_descriptions(tools, "cursor").len(),
            1
        );
    }

    #[test]
    fn summary_works() {
        let _guard = setup();
        let summary = list_tools_opt::schema_opt_summary();

        assert_eq!(summary.optimizations_applied, 0);
        assert_eq!(summary.total_tokens_saved, 0);
        assert_eq!(summary.avg_reduction_percent, 0.0);
    }

    #[test]
    fn optimization_flag_check() {
        let _guard = setup();

        assert!(list_tools_opt::should_optimize_for_client("cursor"));
        assert!(!list_tools_opt::should_optimize_for_client(
            "unknown-client"
        ));
    }
    /// Every tool must keep its OWN description.
    ///
    /// `optimize_schemas` sorts entries by estimated token size and drops them
    /// when over budget, so its output is neither in the input order nor the
    /// same length. This used to be zipped against the original list by
    /// position, which handed each tool someone else's description — and
    /// truncated the advertised surface whenever an entry had been dropped.
    /// Only clients the kernel considers addressable (Cursor, Zed) were hit.
    #[test]
    fn descriptions_stay_with_their_own_tool() {
        let _guard = setup();

        let tools = crate::server::registry::build_registry().tool_defs();
        let before: std::collections::HashMap<String, String> = tools
            .iter()
            .map(|t| {
                (
                    t.name.to_string(),
                    t.description.as_deref().unwrap_or("").to_owned(),
                )
            })
            .collect();

        let after = super::optimize_tools(tools, "cursor");

        assert_eq!(
            after.len(),
            before.len(),
            "optimization must not drop tools from the advertised surface"
        );
        for tool in &after {
            let original = &before[tool.name.as_ref()];
            let got = tool.description.as_deref().unwrap_or("");
            // The optimizer may compress a description, but the result must be
            // derived from that tool's own text — never from another tool's.
            let belongs_elsewhere = before
                .iter()
                .any(|(name, text)| name != tool.name.as_ref() && text == got && text != original);
            assert!(
                !belongs_elsewhere,
                "{} was given another tool's description",
                tool.name
            );
        }
    }

}
