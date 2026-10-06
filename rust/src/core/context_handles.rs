//! Context Handles -- sparse, lazy references to context items.
//!
//! A handle is a lightweight proxy (~5-30 tokens) for a context item that
//! would otherwise cost hundreds or thousands of tokens to include in full.
//! Handles are rendered in a compact manifest inside the system prompt,
//! allowing the agent to load only the items it needs with `ctx_read`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::context_field::{ContextItemId, ContextKind, ViewCosts, ViewKind};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A sparse, lazy reference to a context item.
///
/// The handle carries just enough information for the agent to decide
/// whether to expand the item, without paying the full token cost upfront.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextHandle {
    pub ref_label: String,
    pub item_id: ContextItemId,
    pub kind: ContextKind,
    pub source_path: String,
    pub summary: String,
    pub handle_tokens: usize,
    pub available_views: Vec<(ViewKind, usize)>,
    pub phi: f64,
    pub pinned: bool,
}

/// Registry that owns all active handles and generates sequential ref-labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandleRegistry {
    handles: Vec<ContextHandle>,
    counters: HashMap<ContextKind, usize>,
}

// ---------------------------------------------------------------------------
// Kind -> prefix mapping
// ---------------------------------------------------------------------------

fn kind_prefix(kind: &ContextKind) -> &'static str {
    match kind {
        ContextKind::File => "F",
        ContextKind::Shell => "S",
        ContextKind::Knowledge => "K",
        ContextKind::Memory => "M",
        ContextKind::Provider => "P",
    }
}

// ---------------------------------------------------------------------------
// HandleRegistry implementation
// ---------------------------------------------------------------------------

impl HandleRegistry {
    pub fn new() -> Self {
        Self {
            handles: Vec::new(),
            counters: HashMap::new(),
        }
    }

    /// Register a new context item and assign the next sequential ref-label.
    ///
    /// Returns a reference to the newly created handle.
    pub fn register(
        &mut self,
        item_id: ContextItemId,
        kind: ContextKind,
        source_path: &str,
        summary: &str,
        view_costs: &ViewCosts,
        phi: f64,
        pinned: bool,
    ) -> &ContextHandle {
        let counter = self.counters.entry(kind).or_insert(0);
        *counter += 1;
        let ref_label = format!("{}{}", kind_prefix(&kind), counter);

        let available_views: Vec<(ViewKind, usize)> = {
            let mut views: Vec<_> = view_costs
                .estimates
                .iter()
                .filter(|(v, _)| **v != ViewKind::Handle)
                .map(|(&v, &tokens)| (v, tokens))
                .collect();
            views.sort_by_key(|(v, _)| v.density_rank());
            views
        };

        let handle_tokens = view_costs
            .estimates
            .get(&ViewKind::Handle)
            .copied()
            .unwrap_or_else(|| estimate_handle_tokens(source_path, summary));

        let handle = ContextHandle {
            ref_label,
            item_id,
            kind,
            source_path: source_path.to_string(),
            summary: summary.to_string(),
            handle_tokens,
            available_views,
            phi,
            pinned,
        };

        self.handles.push(handle);
        self.handles.last().expect("just pushed")
    }

    /// Look up a handle by its ref-label (e.g. "F1", "S3").
    ///
    /// Accepts labels with or without the leading `@`.
    pub fn resolve(&self, ref_label: &str) -> Option<&ContextHandle> {
        let label = ref_label.strip_prefix('@').unwrap_or(ref_label);
        self.handles.iter().find(|h| h.ref_label == label)
    }

    /// All registered handles, in registration order.
    pub fn all(&self) -> &[ContextHandle] {
        &self.handles
    }
}

impl Default for HandleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Rough estimate of how many tokens a handle line costs when no explicit
/// Handle view cost is provided. Based on typical tokenizer ratios.
fn estimate_handle_tokens(source_path: &str, summary: &str) -> usize {
    let chars = source_path.len() + summary.len() + 20; // overhead for label, phi, etc.
    (chars / 4).clamp(5, 50)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_view_costs(full_tokens: usize) -> ViewCosts {
        ViewCosts::from_full_tokens(full_tokens)
    }

    #[test]
    fn label_generation_sequential_per_kind() {
        let mut reg = HandleRegistry::new();

        let h1 = reg.register(
            ContextItemId::from_file("a.ts"),
            ContextKind::File,
            "a.ts",
            "module A",
            &sample_view_costs(1000),
            0.9,
            false,
        );
        assert_eq!(h1.ref_label, "F1");

        let h2 = reg.register(
            ContextItemId::from_file("b.ts"),
            ContextKind::File,
            "b.ts",
            "module B",
            &sample_view_costs(500),
            0.8,
            false,
        );
        assert_eq!(h2.ref_label, "F2");

        let h3 = reg.register(
            ContextItemId::from_shell("pytest"),
            ContextKind::Shell,
            "pytest_latest",
            "test run output",
            &sample_view_costs(2000),
            0.7,
            false,
        );
        assert_eq!(h3.ref_label, "S1");

        let h4 = reg.register(
            ContextItemId::from_knowledge("domain", "billing"),
            ContextKind::Knowledge,
            "billing_rules",
            "annual billing assumption",
            &sample_view_costs(100),
            0.95,
            true,
        );
        assert_eq!(h4.ref_label, "K1");
    }

    #[test]
    fn resolve_by_ref_label() {
        let mut reg = HandleRegistry::new();
        reg.register(
            ContextItemId::from_file("x.rs"),
            ContextKind::File,
            "x.rs",
            "file X",
            &sample_view_costs(400),
            0.85,
            false,
        );
        reg.register(
            ContextItemId::from_shell("cargo test"),
            ContextKind::Shell,
            "cargo_test",
            "test output",
            &sample_view_costs(800),
            0.6,
            false,
        );

        assert!(reg.resolve("F1").is_some());
        assert_eq!(reg.resolve("F1").unwrap().source_path, "x.rs");

        assert!(reg.resolve("@S1").is_some());
        assert_eq!(reg.resolve("@S1").unwrap().source_path, "cargo_test");

        assert!(reg.resolve("F99").is_none());
    }

    #[test]
    fn multiple_registrations_sequential() {
        let mut reg = HandleRegistry::new();
        for i in 1..=5 {
            let path = format!("file_{i}.rs");
            let id = ContextItemId::from_file(&path);
            reg.register(
                id,
                ContextKind::File,
                &path,
                "some module",
                &sample_view_costs(500),
                0.5,
                false,
            );
        }

        assert_eq!(reg.all().len(), 5);
        assert_eq!(reg.all()[0].ref_label, "F1");
        assert_eq!(reg.all()[1].ref_label, "F2");
        assert_eq!(reg.all()[2].ref_label, "F3");
        assert_eq!(reg.all()[3].ref_label, "F4");
        assert_eq!(reg.all()[4].ref_label, "F5");
    }

    #[test]
    fn mixed_kinds_independent_counters() {
        let mut reg = HandleRegistry::new();

        reg.register(
            ContextItemId::from_file("a.rs"),
            ContextKind::File,
            "a.rs",
            "file",
            &sample_view_costs(100),
            0.5,
            false,
        );
        reg.register(
            ContextItemId::from_shell("ls"),
            ContextKind::Shell,
            "ls",
            "listing",
            &sample_view_costs(100),
            0.5,
            false,
        );
        reg.register(
            ContextItemId::from_file("b.rs"),
            ContextKind::File,
            "b.rs",
            "file",
            &sample_view_costs(100),
            0.5,
            false,
        );
        reg.register(
            ContextItemId::from_memory("session"),
            ContextKind::Memory,
            "session_state",
            "memory",
            &sample_view_costs(100),
            0.5,
            false,
        );
        reg.register(
            ContextItemId::from_provider("github", "pr"),
            ContextKind::Provider,
            "github/pr/123",
            "pull request",
            &sample_view_costs(100),
            0.5,
            false,
        );

        assert_eq!(reg.resolve("F1").unwrap().source_path, "a.rs");
        assert_eq!(reg.resolve("S1").unwrap().source_path, "ls");
        assert_eq!(reg.resolve("F2").unwrap().source_path, "b.rs");
        assert_eq!(reg.resolve("M1").unwrap().source_path, "session_state");
        assert_eq!(reg.resolve("P1").unwrap().source_path, "github/pr/123");
    }

    #[test]
    fn available_views_sorted_by_density() {
        let mut reg = HandleRegistry::new();
        let h = reg.register(
            ContextItemId::from_file("c.rs"),
            ContextKind::File,
            "c.rs",
            "module C",
            &sample_view_costs(4000),
            0.9,
            false,
        );

        let ranks: Vec<u8> = h
            .available_views
            .iter()
            .map(|(v, _)| v.density_rank())
            .collect();

        for window in ranks.windows(2) {
            assert!(
                window[0] <= window[1],
                "views should be sorted by density rank (dense first)"
            );
        }
    }

    #[test]
    fn handle_tokens_fallback_without_handle_view() {
        let mut costs = ViewCosts::new();
        costs.set(ViewKind::Full, 5000);
        costs.set(ViewKind::Signatures, 1000);

        let mut reg = HandleRegistry::new();
        let h = reg.register(
            ContextItemId::from_file("big.rs"),
            ContextKind::File,
            "src/core/big_module.rs",
            "large module with many exports",
            &costs,
            0.88,
            false,
        );

        assert!(
            h.handle_tokens >= 5,
            "fallback should produce at least 5 tokens"
        );
        assert!(
            h.handle_tokens <= 50,
            "fallback should produce at most 50 tokens"
        );
    }
}
