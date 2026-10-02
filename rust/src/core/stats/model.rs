use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Persistent store for all-time token savings, command stats, and daily history.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct StatsStore {
    pub total_commands: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub first_use: Option<String>,
    pub last_use: Option<String>,
    pub commands: HashMap<String, CommandStats>,
    pub daily: Vec<DayStats>,
    #[serde(default)]
    pub cep: CepStats,
    /// Delivery classification recorded for each command. Older stats files do
    /// not have this map; callers infer classifications from the command key.
    #[serde(default)]
    pub command_classes: HashMap<String, TrafficClass>,
    /// Savings when a compressed tool result first enters provider context.
    #[serde(default)]
    pub first_inject_tokens_saved: u64,
    /// Savings from previously injected tool results carried into later turns.
    #[serde(default)]
    pub reread_tokens_saved: u64,
    /// Savings still resident in the active transcript and eligible for re-read.
    #[serde(default)]
    pub active_tool_result_tokens_saved: u64,
    /// Last provider turn for which active tool-result re-reads were accrued.
    #[serde(default)]
    pub last_tool_result_turn: u64,
    /// Number of results recorded with stream-aware accounting. Zero identifies
    /// legacy stats files whose aggregate savings need a display-time fallback.
    #[serde(default)]
    pub stream_tracked_results: u64,
}

/// Whether a recorded command's output is controlled by compression.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrafficClass {
    Compressible,
    Passthrough,
}

/// Token totals for traffic that lean-ctx can compress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CompressionTotals {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
}

impl CompressionTotals {
    pub(crate) fn saved_tokens(self) -> u64 {
        self.input_tokens.saturating_sub(self.output_tokens)
    }

    pub(crate) fn compression_pct(self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.saved_tokens() as f64 / self.input_tokens as f64 * 100.0
        }
    }
}

impl StatsStore {
    /// Records one tool result at the provider turn that caused it. Advancing
    /// turns prices every already-resident result as a cache re-read; results
    /// emitted in the same turn are not spuriously re-billed against each other.
    pub(crate) fn record_tool_result_savings(&mut self, saved: u64, turn: u64) {
        if turn > 0 && self.last_tool_result_turn > 0 && turn > self.last_tool_result_turn {
            let elapsed = turn - self.last_tool_result_turn;
            self.reread_tokens_saved = self
                .reread_tokens_saved
                .saturating_add(self.active_tool_result_tokens_saved.saturating_mul(elapsed));
        }
        if turn > 0 {
            self.last_tool_result_turn = self.last_tool_result_turn.max(turn);
            self.active_tool_result_tokens_saved =
                self.active_tool_result_tokens_saved.saturating_add(saved);
        }
        self.first_inject_tokens_saved = self.first_inject_tokens_saved.saturating_add(saved);
        self.stream_tracked_results = self.stream_tracked_results.saturating_add(1);
    }

    /// Stream totals through `observed_turn`, including re-reads since the last
    /// tool result. Legacy stores fall back to measured compressible savings.
    pub(crate) fn stream_savings(&self, observed_turn: u64) -> (u64, u64) {
        if self.stream_tracked_results == 0 {
            return (self.compression_totals().saved_tokens(), 0);
        }
        let pending_turns = observed_turn.saturating_sub(self.last_tool_result_turn);
        let pending = self
            .active_tool_result_tokens_saved
            .saturating_mul(pending_turns);
        (
            self.first_inject_tokens_saved,
            self.reread_tokens_saved.saturating_add(pending),
        )
    }

    /// Computes effective compression from tagged command rows.
    pub(crate) fn compression_totals(&self) -> CompressionTotals {
        self.commands
            .iter()
            .filter(|(command, _)| {
                self.command_classes
                    .get(*command)
                    .copied()
                    .unwrap_or_else(|| classify_command(command))
                    == TrafficClass::Compressible
            })
            .fold(CompressionTotals::default(), |mut totals, (_, stats)| {
                totals.input_tokens = totals.input_tokens.saturating_add(stats.input_tokens);
                totals.output_tokens = totals.output_tokens.saturating_add(stats.output_tokens);
                totals
            })
    }

    /// Total reduction across both compressible and passthrough traffic.
    pub(crate) fn total_reduction_pct(&self) -> f64 {
        if self.total_input_tokens == 0 {
            0.0
        } else {
            self.total_input_tokens
                .saturating_sub(self.total_output_tokens) as f64
                / self.total_input_tokens as f64
                * 100.0
        }
    }
}

/// Classifies normalized stats keys, with an explicit passthrough default for
/// control/listing tools so only read, shell, and search output drives the
/// effective-compression denominator.
pub(crate) fn classify_command(command: &str) -> TrafficClass {
    match command {
        "cli_full" | "cli_raw" | "cli_glob" | "cli_find" | "cli_deps" | "cli_ls"
        | "ctx_compose" | "ctx_glob" | "ctx_tree" => TrafficClass::Passthrough,
        c if c.starts_with("cli_") => TrafficClass::Compressible,
        "ctx_shell" | "ctx_search" => TrafficClass::Compressible,
        c if c.starts_with("ctx_read") => TrafficClass::Compressible,
        _ => TrafficClass::Passthrough,
    }
}

/// Aggregated CEP (Cognitive Efficiency Protocol) metrics across sessions.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct CepStats {
    pub sessions: u64,
    pub total_cache_hits: u64,
    pub total_cache_reads: u64,
    pub total_tokens_original: u64,
    pub total_tokens_compressed: u64,
    pub modes: HashMap<String, u64>,
    pub scores: Vec<CepSessionSnapshot>,
    #[serde(default)]
    pub last_session_pid: Option<u32>,
    #[serde(default)]
    pub last_session_original: Option<u64>,
    #[serde(default)]
    pub last_session_compressed: Option<u64>,
    /// Cumulative cache hits/reads observed for the current PID at the last
    /// snapshot. Used to accumulate *deltas* across repeated snapshots within
    /// one server process, so `total_cache_hits` keeps tracking cache activity
    /// after the first checkpoint instead of freezing (#361).
    #[serde(default)]
    pub last_session_cache_hits: Option<u64>,
    #[serde(default)]
    pub last_session_cache_reads: Option<u64>,
}

/// Point-in-time snapshot of CEP scores for a single session.
#[derive(Serialize, Deserialize, Clone)]
pub struct CepSessionSnapshot {
    pub timestamp: String,
    pub score: u32,
    pub cache_hit_rate: u32,
    pub mode_diversity: u32,
    pub compression_rate: u32,
    pub tool_calls: u64,
    pub tokens_saved: u64,
    pub complexity: String,
}

/// Per-command token statistics: invocation count and input/output totals.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct CommandStats {
    pub count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Daily aggregate: command count and token totals for one calendar day.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct DayStats {
    pub date: String,
    pub commands: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// lean-ctx version active when this day's stats were last recorded.
    /// Lets `lean-ctx gain` attribute per-day compression changes to a release
    /// (#307). Empty for days recorded before this field existed.
    #[serde(default)]
    pub version: String,
}

/// High-level token savings summary for display.
pub struct GainSummary {
    pub total_saved: u64,
    pub total_calls: u64,
}

/// Average LLM pricing per 1M tokens (blended across Claude, GPT, Gemini).
pub const DEFAULT_INPUT_PRICE_PER_M: f64 = 2.50;
pub const DEFAULT_OUTPUT_PRICE_PER_M: f64 = 10.0;

/// LLM pricing model for estimating dollar savings from token compression.
pub struct CostModel {
    pub input_price_per_m: f64,
    pub output_price_per_m: f64,
    pub avg_verbose_output_per_call: u64,
    pub avg_concise_output_per_call: u64,
}

impl Default for CostModel {
    fn default() -> Self {
        let env_model = std::env::var("LEAN_CTX_MODEL")
            .or_else(|_| std::env::var("LCTX_MODEL"))
            .ok();
        let pricing = crate::core::gain::model_pricing::ModelPricing::load();
        let quote = pricing.quote(env_model.as_deref());
        Self {
            input_price_per_m: quote.cost.input_per_m,
            output_price_per_m: quote.cost.output_per_m,
            avg_verbose_output_per_call: 180,
            avg_concise_output_per_call: 120,
        }
    }
}

/// Detailed cost comparison: with vs. without lean-ctx compression.
pub struct CostBreakdown {
    pub input_cost_without: f64,
    pub input_cost_with: f64,
    pub output_cost_without: f64,
    pub output_cost_with: f64,
    pub total_cost_without: f64,
    pub total_cost_with: f64,
    pub total_saved: f64,
    pub estimated_output_tokens_without: u64,
    pub estimated_output_tokens_with: u64,
    pub output_tokens_saved: u64,
}

impl CostModel {
    /// Calculates the full cost breakdown from the stats store.
    pub fn calculate(&self, store: &StatsStore) -> CostBreakdown {
        let input_cost_without =
            store.total_input_tokens as f64 / 1_000_000.0 * self.input_price_per_m;
        let input_cost_with =
            store.total_output_tokens as f64 / 1_000_000.0 * self.input_price_per_m;

        let input_saved = store
            .total_input_tokens
            .saturating_sub(store.total_output_tokens);
        let compression_rate = if store.total_input_tokens > 0 {
            input_saved as f64 / store.total_input_tokens as f64
        } else {
            0.0
        };
        let est_output_without = store.total_commands * self.avg_verbose_output_per_call;
        let est_output_with = if compression_rate > 0.01 {
            store.total_commands * self.avg_concise_output_per_call
        } else {
            est_output_without
        };
        let output_saved = est_output_without.saturating_sub(est_output_with);

        let output_cost_without = est_output_without as f64 / 1_000_000.0 * self.output_price_per_m;
        let output_cost_with = est_output_with as f64 / 1_000_000.0 * self.output_price_per_m;

        let total_without = input_cost_without + output_cost_without;
        let total_with = input_cost_with + output_cost_with;

        CostBreakdown {
            input_cost_without,
            input_cost_with,
            output_cost_without,
            output_cost_with,
            total_cost_without: total_without,
            total_cost_with: total_with,
            total_saved: total_without - total_with,
            estimated_output_tokens_without: est_output_without,
            estimated_output_tokens_with: est_output_with,
            output_tokens_saved: output_saved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CompressionTotals, StatsStore, TrafficClass, classify_command};

    #[test]
    fn known_tools_are_classified_by_delivery_contract() {
        assert_eq!(classify_command("ctx_read"), TrafficClass::Compressible);
        assert_eq!(classify_command("ctx_shell"), TrafficClass::Compressible);
        assert_eq!(classify_command("ctx_search"), TrafficClass::Compressible);
        assert_eq!(classify_command("ctx_compose"), TrafficClass::Passthrough);
        assert_eq!(classify_command("ctx_glob"), TrafficClass::Passthrough);
        assert_eq!(classify_command("cli_full"), TrafficClass::Passthrough);
    }

    #[test]
    fn compression_totals_fall_back_for_legacy_stats() {
        let mut store = StatsStore::default();
        store.commands.insert(
            "ctx_read".into(),
            super::CommandStats {
                count: 1,
                input_tokens: 1_000,
                output_tokens: 400,
            },
        );
        store.commands.insert(
            "ctx_glob".into(),
            super::CommandStats {
                count: 1,
                input_tokens: 500,
                output_tokens: 500,
            },
        );

        assert_eq!(store.compression_totals().saved_tokens(), 600);
        assert_eq!(store.compression_totals().compression_pct(), 60.0);
    }

    #[test]
    fn explicit_command_tag_overrides_legacy_inference() {
        let mut store = StatsStore::default();
        store.commands.insert(
            "custom_tool".into(),
            super::CommandStats {
                count: 1,
                input_tokens: 100,
                output_tokens: 25,
            },
        );
        store
            .command_classes
            .insert("custom_tool".into(), TrafficClass::Compressible);

        assert_eq!(store.compression_totals().input_tokens, 100);
        assert_eq!(store.total_reduction_pct(), 0.0);
    }

    #[test]
    fn compression_totals_handle_zero_input_without_nan() {
        let totals = CompressionTotals::default();
        assert_eq!(totals.saved_tokens(), 0);
        assert_eq!(totals.compression_pct(), 0.0);
        assert_eq!(StatsStore::default().total_reduction_pct(), 0.0);
    }

    #[test]
    fn first_result_is_first_inject_only() {
        let mut store = StatsStore::default();
        store.record_tool_result_savings(1_000, 7);
        assert_eq!(store.stream_savings(7), (1_000, 0));
        assert_eq!(store.last_tool_result_turn, 7);
    }

    #[test]
    fn next_turn_rereads_all_prior_results() {
        let mut store = StatsStore::default();
        store.record_tool_result_savings(1_000, 7);
        store.record_tool_result_savings(500, 8);
        assert_eq!(store.stream_savings(8), (1_500, 1_000));
    }

    #[test]
    fn parallel_results_on_same_turn_do_not_reread_each_other() {
        let mut store = StatsStore::default();
        store.record_tool_result_savings(1_000, 7);
        store.record_tool_result_savings(500, 7);
        assert_eq!(store.stream_savings(7), (1_500, 0));
        assert_eq!(store.stream_savings(8), (1_500, 1_500));
    }

    #[test]
    fn skipped_turns_multiply_resident_savings() {
        let mut store = StatsStore::default();
        store.record_tool_result_savings(2_000, 3);
        assert_eq!(store.stream_savings(6), (2_000, 6_000));
    }

    #[test]
    fn daemon_free_result_never_guesses_rereads() {
        let mut store = StatsStore::default();
        store.record_tool_result_savings(2_000, 0);
        assert_eq!(store.stream_savings(100), (2_000, 0));
        assert_eq!(store.active_tool_result_tokens_saved, 0);
    }

    #[test]
    fn legacy_stats_fall_back_to_effective_compression() {
        let mut store = StatsStore::default();
        store.commands.insert(
            "ctx_read".into(),
            super::CommandStats {
                count: 1,
                input_tokens: 10_000,
                output_tokens: 2_500,
            },
        );
        assert_eq!(store.stream_savings(50), (7_500, 0));
    }

    #[test]
    fn stream_counters_saturate_instead_of_wrapping() {
        let mut store = StatsStore::default();
        store.record_tool_result_savings(u64::MAX, 1);
        store.record_tool_result_savings(u64::MAX, u64::MAX);
        assert_eq!(store.first_inject_tokens_saved, u64::MAX);
        assert_eq!(store.reread_tokens_saved, u64::MAX);
    }
}
