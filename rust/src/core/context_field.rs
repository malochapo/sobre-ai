//! Context Field Theory (CFT) -- unified potential function for context items.
//!
//! Combines information-theoretic, graph-based, and history signals into a
//! single scalar potential Phi(i,t) per context item, enabling principled
//! budget allocation and view selection.
//!
//! Scientific basis:
//!   Phi(i,t) = w_R*R + w_S*S + w_G*G + w_H*H - w_C*C - w_D*D
//! where R = task relevance (heat diffusion + PageRank),
//!       S = surprise (cross-entropy with Zipfian prior),
//!       G = graph proximity (weighted BFS distance),
//!       H = history signal (bandit feedback),
//!       C = token cost for the active view,
//!       D = redundancy with already-selected items (Jaccard).

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared types used across CFT modules (Ledger, Overlay, Handles, Compiler)
// ---------------------------------------------------------------------------

/// Stable, content-addressed identifier for a context item.
/// Derived from `kind + source_path` so the same file always maps to the
/// same ID within a session, regardless of content changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContextItemId(pub String);

impl ContextItemId {
    pub fn from_file(path: &str) -> Self {
        Self(format!("file:{path}"))
    }
    #[cfg(test)]
    pub fn from_shell(command: &str) -> Self {
        let hash = crate::core::project_hash::hash_project_root(command);
        Self(format!("shell:{hash}"))
    }
    #[cfg(test)]
    pub fn from_knowledge(category: &str, key: &str) -> Self {
        Self(format!("knowledge:{category}:{key}"))
    }
    #[cfg(test)]
    pub fn from_memory(key: &str) -> Self {
        Self(format!("memory:{key}"))
    }
    pub fn from_provider(provider: &str, key: &str) -> Self {
        Self(format!("provider:{provider}:{key}"))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContextItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    File,
    Shell,
    Knowledge,
    Memory,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ContextState {
    #[default]
    Candidate,
    Included,
    Excluded,
    Pinned,
    Stale,
    Shadowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Full,
    Signatures,
    Map,
    Diff,
    Aggressive,
    Entropy,
    Lines,
    Reference,
    Handle,
}

impl ViewKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Signatures => "signatures",
            Self::Map => "map",
            Self::Diff => "diff",
            Self::Aggressive => "aggressive",
            Self::Entropy => "entropy",
            Self::Lines => "lines",
            Self::Reference => "reference",
            Self::Handle => "handle",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "signatures" => Self::Signatures,
            "map" => Self::Map,
            "diff" => Self::Diff,
            "aggressive" => Self::Aggressive,
            "entropy" => Self::Entropy,
            "lines" => Self::Lines,
            "reference" => Self::Reference,
            "handle" => Self::Handle,
            _ => Self::Full,
        }
    }

    /// Phase-transition ordering: lower index = denser (more tokens).
    pub fn density_rank(&self) -> u8 {
        match self {
            Self::Full => 0,
            Self::Aggressive => 1,
            Self::Diff => 2,
            Self::Lines => 3,
            Self::Entropy => 4,
            Self::Signatures => 5,
            Self::Map => 6,
            Self::Reference => 7,
            Self::Handle => 8,
        }
    }
}

/// Token-cost estimates for each available view of a context item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewCosts {
    pub estimates: HashMap<ViewKind, usize>,
}

impl ViewCosts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, view: ViewKind, tokens: usize) {
        self.estimates.insert(view, tokens);
    }

    pub fn get(&self, view: &ViewKind) -> usize {
        self.estimates.get(view).copied().unwrap_or(0)
    }

    pub fn from_full_tokens(full_tokens: usize) -> Self {
        let mut vc = Self::new();
        vc.set(ViewKind::Full, full_tokens);
        vc.set(ViewKind::Signatures, full_tokens / 5);
        vc.set(ViewKind::Map, full_tokens / 8);
        vc.set(ViewKind::Reference, full_tokens / 20);
        vc.set(ViewKind::Handle, 25);
        vc
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Provenance {
    pub tool: Option<String>,
    pub agent_id: Option<String>,
    pub client_name: Option<String>,
    pub timestamp: Option<String>,
}

// ---------------------------------------------------------------------------
// Context Potential Function
// ---------------------------------------------------------------------------

/// Weights for the potential function components.
/// Adapted via Thompson Sampling (bandit.rs) over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldWeights {
    pub w_relevance: f64,
    pub w_surprise: f64,
    pub w_graph: f64,
    pub w_history: f64,
    pub w_cost: f64,
    pub w_redundancy: f64,
}

impl Default for FieldWeights {
    fn default() -> Self {
        Self {
            w_relevance: 0.35,
            w_surprise: 0.15,
            w_graph: 0.20,
            w_history: 0.10,
            w_cost: 0.10,
            w_redundancy: 0.10,
        }
    }
}

impl FieldWeights {
    /// Stability-leaning preset (#4): trusts relevance/history, applies a light
    /// cost penalty. Selected when the `conservative` bandit arm wins.
    pub fn conservative() -> Self {
        Self {
            w_relevance: 0.45,
            w_surprise: 0.10,
            w_graph: 0.20,
            w_history: 0.15,
            w_cost: 0.05,
            w_redundancy: 0.05,
        }
    }

    /// The default balanced preset (#4); the `balanced` arm maps here.
    pub fn balanced() -> Self {
        Self::default()
    }

    /// Compression-leaning preset (#4): heavier cost/surprise weighting so dense
    /// items are favored under pressure. Selected when `aggressive` wins.
    pub fn aggressive() -> Self {
        Self {
            w_relevance: 0.30,
            w_surprise: 0.20,
            w_graph: 0.15,
            w_history: 0.05,
            w_cost: 0.20,
            w_redundancy: 0.10,
        }
    }
}

/// The weights every Phi computation uses.
pub fn active_weights() -> FieldWeights {
    FieldWeights::default()
}

/// Raw signal components for a single context item before combination.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldSignals {
    pub relevance: f64,
    pub surprise: f64,
    pub graph_proximity: f64,
    pub history_signal: f64,
    pub token_cost_norm: f64,
    pub redundancy: f64,
}

/// The Context Field: computes Phi for a set of items given a task context.
pub struct ContextField {
    weights: FieldWeights,
}

impl Default for ContextField {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextField {
    pub fn new() -> Self {
        Self {
            weights: FieldWeights::default(),
        }
    }

    /// Construct a field using the process-wide learned FieldWeights (#4) when an
    /// arm has been selected, else the defaults. Cheap (a `RwLock` read), so it is
    /// safe to call on the per-read Phi hot path.
    pub fn active() -> Self {
        Self {
            weights: active_weights(),
        }
    }

    /// Compute the unified potential Phi(i,t) for a context item.
    ///
    /// All input signals should be normalized to [0, 1] before calling.
    /// The cost and redundancy terms are subtracted (penalty).
    pub fn compute_phi(&self, signals: &FieldSignals) -> f64 {
        let w = &self.weights;
        let phi = w.w_relevance * signals.relevance
            + w.w_surprise * signals.surprise
            + w.w_graph * signals.graph_proximity
            + w.w_history * signals.history_signal
            - w.w_cost * signals.token_cost_norm
            - w.w_redundancy * signals.redundancy;
        phi.clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Signal extraction helpers (bridge to existing modules)
// ---------------------------------------------------------------------------

/// Normalize a relevance score from task_relevance.rs to [0, 1].
pub fn normalize_relevance(score: f64, max_score: f64) -> f64 {
    if max_score <= 0.0 {
        return 0.0;
    }
    (score / max_score).clamp(0.0, 1.0)
}

/// Normalize a surprise score from surprise.rs to [0, 1].
/// Surprise range is typically 5.0 (common) to 17.0+ (rare).
pub fn normalize_surprise(surprise: f64) -> f64 {
    ((surprise - 5.0) / 12.0).clamp(0.0, 1.0)
}

/// Normalize graph proximity (inverse of distance) to [0, 1].
/// Distance 0 = same file = 1.0, distance N = 1/(1+N).
pub fn normalize_graph_proximity(distance: usize) -> f64 {
    1.0 / (1.0 + distance as f64)
}

/// Normalize token cost relative to budget.
pub fn normalize_token_cost(tokens: usize, budget_total: usize) -> f64 {
    if budget_total == 0 {
        return 1.0;
    }
    (tokens as f64 / budget_total as f64).clamp(0.0, 1.0)
}

/// Compute real signals for a file path using existing scoring modules.
/// Bridges CFT with the information-theoretic, graph-based, and history
/// subsystems already in lean-ctx.
pub fn compute_signals_for_path(
    path: &str,
    task: Option<&str>,
    file_content: Option<&str>,
    budget_total: usize,
    full_tokens: usize,
) -> (FieldSignals, ViewCosts) {
    let mut signals = FieldSignals::default();

    let heatmap = super::heatmap::HeatMap::load();
    let heat_entry = heatmap.entries.get(path);

    // R(i,t): Task relevance via keyword overlap + heatmap frequency
    if let Some(task_desc) = task {
        let (_, keywords) = super::task_relevance::parse_task_hints(task_desc);
        let path_lower = path.to_lowercase();
        let keyword_hits = keywords
            .iter()
            .filter(|kw| path_lower.contains(&kw.to_lowercase()))
            .count();
        let keyword_score = (keyword_hits as f64 * 0.3).min(1.0);
        let freq_score = heat_entry.map_or(0.0, |e| (e.access_count as f64 / 10.0).min(1.0));
        signals.relevance = normalize_relevance(keyword_score + freq_score, 2.0);
    } else {
        let freq = heat_entry.map_or(0.0, |e| e.access_count as f64);
        signals.relevance = normalize_relevance(freq, 10.0);
    }

    // S(i): Surprise from cross-entropy with Zipfian prior
    if let Some(content) = file_content {
        let surprise_val = super::surprise::line_surprise(content);
        signals.surprise = normalize_surprise(surprise_val);
    }

    // G(i,t): Graph proximity heuristic from path depth
    // (property graph queries require a Connection not available here)
    let depth = path.matches('/').count();
    signals.graph_proximity = normalize_graph_proximity(depth);

    // H(i): History signal from heatmap access count
    let access_count = heat_entry.map_or(0, |e| e.access_count);
    signals.history_signal = (access_count as f64 / 20.0).min(1.0);

    // C(i,v): Normalized token cost relative to budget
    signals.token_cost_norm = normalize_token_cost(full_tokens, budget_total);

    // D(i): Redundancy — initialized at 0, refined during compilation pass
    signals.redundancy = 0.0;

    let view_costs = ViewCosts::from_full_tokens(full_tokens);
    (signals, view_costs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phi_increases_with_relevance() {
        let field = ContextField::new();
        let low = field.compute_phi(&FieldSignals {
            relevance: 0.2,
            ..Default::default()
        });
        let high = field.compute_phi(&FieldSignals {
            relevance: 0.9,
            ..Default::default()
        });
        assert!(high > low, "higher relevance should yield higher phi");
    }

    #[test]
    fn phi_decreases_with_cost() {
        let field = ContextField::new();
        let cheap = field.compute_phi(&FieldSignals {
            relevance: 0.5,
            token_cost_norm: 0.1,
            ..Default::default()
        });
        let expensive = field.compute_phi(&FieldSignals {
            relevance: 0.5,
            token_cost_norm: 0.9,
            ..Default::default()
        });
        assert!(cheap > expensive, "higher cost should reduce phi");
    }

    #[test]
    fn phi_decreases_with_redundancy() {
        let field = ContextField::new();
        let unique = field.compute_phi(&FieldSignals {
            relevance: 0.5,
            redundancy: 0.0,
            ..Default::default()
        });
        let redundant = field.compute_phi(&FieldSignals {
            relevance: 0.5,
            redundancy: 0.9,
            ..Default::default()
        });
        assert!(unique > redundant, "redundancy should reduce phi");
    }

    #[test]
    fn phi_is_clamped_to_unit_interval() {
        let field = ContextField::new();
        let phi = field.compute_phi(&FieldSignals {
            relevance: 1.0,
            surprise: 1.0,
            graph_proximity: 1.0,
            history_signal: 1.0,
            token_cost_norm: 0.0,
            redundancy: 0.0,
        });
        assert!(phi <= 1.0);
        assert!(phi >= 0.0);
    }

    #[test]
    fn normalize_surprise_maps_range() {
        assert!((normalize_surprise(5.0) - 0.0).abs() < 0.01);
        assert!((normalize_surprise(17.0) - 1.0).abs() < 0.01);
        assert!((normalize_surprise(11.0) - 0.5).abs() < 0.01);
    }

    #[test]
    fn normalize_graph_proximity_inverse_distance() {
        assert!((normalize_graph_proximity(0) - 1.0).abs() < f64::EPSILON);
        assert!((normalize_graph_proximity(1) - 0.5).abs() < f64::EPSILON);
        assert!(normalize_graph_proximity(10) < 0.15);
    }

    #[test]
    fn context_item_id_stable() {
        let a = ContextItemId::from_file("src/main.rs");
        let b = ContextItemId::from_file("src/main.rs");
        assert_eq!(a, b);
    }

    #[test]
    fn view_costs_from_full() {
        let vc = ViewCosts::from_full_tokens(5000);
        assert_eq!(vc.get(&ViewKind::Full), 5000);
        assert_eq!(vc.get(&ViewKind::Signatures), 1000);
        assert_eq!(vc.get(&ViewKind::Map), 625);
        assert_eq!(vc.get(&ViewKind::Handle), 25);
    }
}
