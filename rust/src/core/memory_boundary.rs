use serde::{Deserialize, Serialize};

/// Visibility of a knowledge fact. Facts are scoped to their originating
/// project; knowledge search never crosses project boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FactPrivacy {
    #[default]
    ProjectOnly,
    LinkedProjects,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BoundaryPolicy {
    /// Controls whether universal (cross-project) gotchas are loaded.
    /// When false, only project-scoped gotchas are used.
    pub universal_gotchas_enabled: bool,
}

impl Default for BoundaryPolicy {
    fn default() -> Self {
        Self {
            universal_gotchas_enabled: true,
        }
    }
}
