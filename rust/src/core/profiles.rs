//! # Context Profiles
//!
//! Declarative, version-controlled context strategies ("Context as Code").
//!
//! Profiles configure how lean-ctx processes content for different scenarios:
//! exploration, bugfixing, hotfixes, CI debugging, code review, etc.
//!
//! ## Resolution Order
//!
//! 1. `LEAN_CTX_PROFILE` env var
//! 2. `.lean-ctx/profiles/<name>.toml` (project-local)
//! 3. `~/.lean-ctx/profiles/<name>.toml` (global)
//! 4. Built-in defaults (compiled into the binary)
//!
//! ## Inheritance
//!
//! Profiles can inherit from other profiles via `inherits = "parent"`.
//! Child values override parent values; unset fields fall through.

use serde::{Deserialize, Serialize};

/// A complete context profile definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub profile: ProfileMeta,
    #[serde(default)]
    pub read: ReadConfig,
    #[serde(default)]
    pub compression: CompressionConfig,
    #[serde(default)]
    pub translation: TranslationConfig,
    #[serde(default)]
    pub layout: LayoutConfig,
    #[serde(default)]
    pub memory: crate::core::memory_policy::MemoryPolicyOverrides,
    #[serde(default)]
    pub verification: crate::core::output_verification::VerificationConfig,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub degradation: DegradationConfig,
    #[serde(default)]
    pub autonomy: ProfileAutonomy,
    #[serde(default)]
    pub output_hints: OutputHints,
}

/// Profile identity and inheritance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileMeta {
    #[serde(default)]
    pub name: String,
    pub inherits: Option<String>,
    #[serde(default)]
    pub description: String,
}

/// Read behavior configuration.
///
/// Fields are `Option<T>` for field-level profile inheritance.
/// Use `_effective()` methods to get the resolved value with defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ReadConfig {
    pub default_mode: Option<String>,
    pub max_tokens_per_file: Option<usize>,
    pub prefer_cache: Option<bool>,
}

impl ReadConfig {
    pub fn default_mode_effective(&self) -> &str {
        self.default_mode.as_deref().unwrap_or("auto")
    }
    pub fn max_tokens_per_file_effective(&self) -> usize {
        self.max_tokens_per_file.unwrap_or(50_000)
    }
    pub fn prefer_cache_effective(&self) -> bool {
        self.prefer_cache.unwrap_or(false)
    }
}

/// Compression strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CompressionConfig {
    /// Enable adaptive compression depth (#1195). When true, compression
    /// aggressiveness is reduced dynamically based on bounce rate and session
    /// length. Default: true.
    pub adaptive: Option<bool>,
    pub crp_mode: Option<String>,
    pub output_density: Option<String>,
    pub entropy_threshold: Option<f64>,
    pub terse_mode: Option<bool>,
}

impl CompressionConfig {
    pub fn crp_mode_effective(&self) -> &str {
        self.crp_mode.as_deref().unwrap_or("tdd")
    }
    pub fn output_density_effective(&self) -> &str {
        self.output_density.as_deref().unwrap_or("normal")
    }
    pub fn entropy_threshold_effective(&self) -> f64 {
        self.entropy_threshold.unwrap_or(0.3)
    }
    pub fn terse_mode_effective(&self) -> bool {
        self.terse_mode.unwrap_or(false)
    }
    pub fn adaptive_effective(&self) -> bool {
        self.adaptive.unwrap_or(true)
    }
}

/// Translation (tokenizer-aware) configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TranslationConfig {
    /// If false, preserve legacy CRP/TDD formats without post-translation.
    pub enabled: Option<bool>,
    /// legacy|ascii|auto
    pub ruleset: Option<String>,
}

impl TranslationConfig {
    pub fn enabled_effective(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
    pub fn ruleset_effective(&self) -> &str {
        self.ruleset.as_deref().unwrap_or("legacy")
    }
}

/// Layout (attention-aware reorder) configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LayoutConfig {
    /// If false, preserve original order.
    pub enabled: Option<bool>,
    /// Minimum line count for enabling reorder.
    pub min_lines: Option<usize>,
}

impl LayoutConfig {
    pub fn enabled_effective(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
    pub fn min_lines_effective(&self) -> usize {
        self.min_lines.unwrap_or(15)
    }
}

/// Routing policy overrides (intent → model tier → read mode/budgets).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoutingConfig {
    /// Hard cap for recommended model tier: fast|standard|premium.
    #[serde(default)]
    pub max_model_tier: Option<String>,
    /// If true, apply deterministic routing degradation under budget/pressure.
    #[serde(default)]
    pub degrade_under_pressure: Option<bool>,
}

impl RoutingConfig {
    pub fn max_model_tier_effective(&self) -> &str {
        self.max_model_tier.as_deref().unwrap_or("premium")
    }

    pub fn degrade_under_pressure_effective(&self) -> bool {
        self.degrade_under_pressure.unwrap_or(true)
    }
}

/// Budget/SLO degradation policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DegradationConfig {
    /// If true, enforce throttling/blocking decisions. Default is warn-only.
    #[serde(default)]
    pub enforce: Option<bool>,
    /// Throttle duration (ms) when policy verdict is Throttle. Default: 250ms.
    #[serde(default)]
    pub throttle_ms: Option<u64>,
}

impl DegradationConfig {
    pub fn enforce_effective(&self) -> bool {
        self.enforce.unwrap_or(false)
    }

    pub fn throttle_ms_effective(&self) -> u64 {
        self.throttle_ms.unwrap_or(250)
    }
}

/// Controls which optional hints/footers are appended to tool output.
/// All default to `false` for minimal output overhead.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputHints {
    pub compressed_hint: Option<bool>,
    pub archive_hint: Option<bool>,
    pub verify_footer: Option<bool>,
    pub related_hint: Option<bool>,
    pub semantic_hint: Option<bool>,
    pub elicitation_hint: Option<bool>,
    pub checkpoint_in_output: Option<bool>,
    pub graph_context_block: Option<bool>,
    pub efficiency_hint: Option<bool>,
}

impl OutputHints {
    pub fn compressed_hint(&self) -> bool {
        self.compressed_hint.unwrap_or(false)
    }
    pub fn archive_hint(&self) -> bool {
        self.archive_hint.unwrap_or(false)
    }
    pub fn verify_footer(&self) -> bool {
        self.verify_footer.unwrap_or(false)
    }
    pub fn related_hint(&self) -> bool {
        self.related_hint.unwrap_or(false)
    }
    pub fn semantic_hint(&self) -> bool {
        self.semantic_hint.unwrap_or(false)
    }
    pub fn elicitation_hint(&self) -> bool {
        self.elicitation_hint.unwrap_or(false)
    }
    pub fn checkpoint_in_output(&self) -> bool {
        self.checkpoint_in_output.unwrap_or(false)
    }
    pub fn graph_context_block(&self) -> bool {
        self.graph_context_block.unwrap_or(false)
    }
    pub fn efficiency_hint(&self) -> bool {
        self.efficiency_hint.unwrap_or(false)
    }
}

/// Token and cost budget limits.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BudgetConfig {
    pub max_context_tokens: Option<usize>,
    pub max_shell_invocations: Option<usize>,
    pub max_cost_usd: Option<f64>,
}

impl BudgetConfig {
    pub fn max_context_tokens_effective(&self) -> usize {
        self.max_context_tokens.unwrap_or(200_000)
    }
    pub fn max_shell_invocations_effective(&self) -> usize {
        self.max_shell_invocations.unwrap_or(100)
    }
    pub fn max_cost_usd_effective(&self) -> f64 {
        self.max_cost_usd.unwrap_or(5.0)
    }
}

/// Pipeline layer activation per profile.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PipelineConfig {
    pub intent: Option<bool>,
    pub relevance: Option<bool>,
    pub compression: Option<bool>,
    pub translation: Option<bool>,
}

impl PipelineConfig {
    pub fn intent_effective(&self) -> bool {
        self.intent.unwrap_or(true)
    }
    pub fn relevance_effective(&self) -> bool {
        self.relevance.unwrap_or(true)
    }
    pub fn compression_effective(&self) -> bool {
        self.compression.unwrap_or(true)
    }
    pub fn translation_effective(&self) -> bool {
        self.translation.unwrap_or(true)
    }
}

/// Autonomy overrides per profile.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProfileAutonomy {
    pub enabled: Option<bool>,
    pub auto_preload: Option<bool>,
    pub auto_dedup: Option<bool>,
    pub auto_related: Option<bool>,
    pub silent_preload: Option<bool>,
    /// Enable bounded prefetch after reads (opt-in by default).
    pub auto_prefetch: Option<bool>,
    /// Enable response shaping for large outputs (opt-in by default).
    pub auto_response: Option<bool>,
    pub dedup_threshold: Option<usize>,
    pub prefetch_max_files: Option<usize>,
    pub prefetch_budget_tokens: Option<usize>,
    pub response_min_tokens: Option<usize>,
    pub checkpoint_interval: Option<u32>,
}

impl ProfileAutonomy {
    pub fn enabled_effective(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
    pub fn auto_preload_effective(&self) -> bool {
        self.auto_preload.unwrap_or(true)
    }
    pub fn auto_dedup_effective(&self) -> bool {
        self.auto_dedup.unwrap_or(true)
    }
    pub fn auto_related_effective(&self) -> bool {
        self.auto_related.unwrap_or(true)
    }
    pub fn silent_preload_effective(&self) -> bool {
        self.silent_preload.unwrap_or(true)
    }
    pub fn auto_prefetch_effective(&self) -> bool {
        self.auto_prefetch.unwrap_or(false)
    }
    pub fn auto_response_effective(&self) -> bool {
        self.auto_response.unwrap_or(false)
    }
    pub fn dedup_threshold_effective(&self) -> usize {
        self.dedup_threshold.unwrap_or(8)
    }
    pub fn prefetch_max_files_effective(&self) -> usize {
        self.prefetch_max_files.unwrap_or(3)
    }
    pub fn prefetch_budget_tokens_effective(&self) -> usize {
        self.prefetch_budget_tokens.unwrap_or(4000)
    }
    pub fn response_min_tokens_effective(&self) -> usize {
        self.response_min_tokens.unwrap_or(600)
    }
    pub fn checkpoint_interval_effective(&self) -> u32 {
        self.checkpoint_interval.unwrap_or(15)
    }
}

// ── Built-in Profiles ──────────────────────────────────────

fn builtin_coder() -> Profile {
    Profile {
        profile: ProfileMeta {
            name: "coder".to_string(),
            inherits: None,
            description: "Default coding workflow with guarded autonomy drivers".to_string(),
        },
        read: ReadConfig {
            default_mode: Some("auto".to_string()),
            max_tokens_per_file: Some(50_000),
            prefer_cache: Some(true),
        },
        compression: CompressionConfig {
            crp_mode: Some("tdd".to_string()),
            output_density: Some("terse".to_string()),
            terse_mode: Some(true),
            ..CompressionConfig::default()
        },
        translation: TranslationConfig {
            enabled: Some(true),
            ruleset: Some("auto".to_string()),
        },
        layout: LayoutConfig::default(),
        memory: crate::core::memory_policy::MemoryPolicyOverrides::default(),
        verification: crate::core::output_verification::VerificationConfig::default(),
        budget: BudgetConfig {
            max_context_tokens: Some(150_000),
            max_shell_invocations: Some(100),
            ..BudgetConfig::default()
        },
        pipeline: PipelineConfig::default(),
        routing: RoutingConfig::default(),
        degradation: DegradationConfig::default(),
        autonomy: ProfileAutonomy {
            auto_prefetch: Some(true),
            auto_response: Some(true),
            checkpoint_interval: Some(10),
            ..ProfileAutonomy::default()
        },
        output_hints: OutputHints::default(),
    }
}




/// The one profile's name.
#[must_use]
pub fn active_profile_name() -> String {
    "coder".to_string()
}

/// The one profile.
///
/// Resolution ran `LEAN_CTX_PROFILE` > config > a `<profiles_dir>/<name>.toml`
/// file with `inherits =` chaining — the cascade invariant 2 forbids.
/// `coder` was the default and sets `RoutingConfig::default()` and
/// `DegradationConfig::default()`, the only two sections anything reads for
/// behaviour, so fixing it here changes nothing that was not deliberate.
#[must_use]
pub fn active_profile() -> Profile {
    builtin_coder()
}



/// Information about an available profile.
#[derive(Debug, Clone)]
pub struct ProfileInfo {
    pub name: String,
    pub description: String,
    pub source: ProfileSource,
}

/// Where a profile was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Builtin,
    Global,
    Project,
}

impl std::fmt::Display for ProfileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => write!(f, "built-in"),
            Self::Global => write!(f, "global"),
            Self::Project => write!(f, "project"),
        }
    }
}


// ── Tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;










    #[test]
    fn active_profile_defaults_to_coder() {
        let _lock = crate::core::data_dir::test_env_lock();
        crate::test_env::remove_var("LEAN_CTX_PROFILE");
        let p = active_profile();
        assert_eq!(p.profile.name, "coder");
    }

    /// Invariant 2: no cascade. `LEAN_CTX_PROFILE` used to select another
    /// profile; it must not move anything now.
    #[test]
    fn env_no_longer_selects_a_profile() {
        let _lock = crate::core::data_dir::test_env_lock();
        crate::test_env::set_var("LEAN_CTX_PROFILE", "hotfix");
        let name = active_profile_name();
        crate::test_env::remove_var("LEAN_CTX_PROFILE");
        assert_eq!(name, "coder");
    }

    #[test]
    fn profile_source_display() {
        assert_eq!(ProfileSource::Builtin.to_string(), "built-in");
        assert_eq!(ProfileSource::Global.to_string(), "global");
        assert_eq!(ProfileSource::Project.to_string(), "project");
    }

    #[test]
    fn default_profile_has_sane_values() {
        let p = Profile {
            profile: ProfileMeta::default(),
            read: ReadConfig::default(),
            compression: CompressionConfig::default(),
            translation: TranslationConfig::default(),
            layout: LayoutConfig::default(),
            memory: crate::core::memory_policy::MemoryPolicyOverrides::default(),
            verification: crate::core::output_verification::VerificationConfig::default(),
            budget: BudgetConfig::default(),
            pipeline: PipelineConfig::default(),
            routing: RoutingConfig::default(),
            degradation: DegradationConfig::default(),
            autonomy: ProfileAutonomy::default(),
            output_hints: OutputHints::default(),
        };
        assert_eq!(p.read.default_mode_effective(), "auto");
        assert_eq!(p.compression.crp_mode_effective(), "tdd");
        assert_eq!(p.budget.max_context_tokens_effective(), 200_000);
        assert!(p.pipeline.compression_effective());
        assert!(p.pipeline.intent_effective());
    }

    #[test]
    fn pipeline_layers_configurable() {
        let toml_str = r#"
[profile]
name = "no-intent"

[pipeline]
intent = false
relevance = false
"#;
        let p: Profile = toml::from_str(toml_str).unwrap();
        assert!(!p.pipeline.intent_effective());
        assert!(!p.pipeline.relevance_effective());
        assert!(p.pipeline.compression_effective());
        assert!(p.pipeline.translation_effective());
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let toml_str = r#"
[profile]
name = "minimal"

[read]
default_mode = "entropy"
"#;
        let p: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(p.read.default_mode_effective(), "entropy");
        assert_eq!(p.read.max_tokens_per_file_effective(), 50_000);
        assert_eq!(p.budget.max_context_tokens_effective(), 200_000);
        assert_eq!(p.compression.crp_mode_effective(), "tdd");
    }

    #[test]
    fn partial_toml_leaves_unset_as_none() {
        let toml_str = r#"
[profile]
name = "sparse"

[read]
default_mode = "map"
"#;
        let p: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(p.read.default_mode, Some("map".to_string()));
        assert_eq!(p.read.max_tokens_per_file, None);
        assert_eq!(p.read.prefer_cache, None);
        assert_eq!(p.budget.max_context_tokens, None);
        assert_eq!(p.compression.crp_mode, None);
    }
}
