//! Deterministic paired-session A/B replay for compression quality (#1192).

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::core::bounce_tracker::BounceTracker;
use crate::core::pricing::{ModelPricing, PricingMatchKind};

const REPLAY_KIND: &str = "lean-ctx.quality-benchmark.v1";
const MAX_REPLAY_BYTES: u64 = 16 * 1024 * 1024;
const Z_95: f64 = 1.959_963_984_540_054;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaySuite {
    pub kind: String,
    pub sessions: Vec<RecordedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedSession {
    pub id: String,
    pub model: String,
    pub without_compression: RecordedArm,
    pub with_compression: RecordedArm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedArm {
    pub success: bool,
    pub turns: u64,
    pub usage: TokenUsage,
    #[serde(default)]
    pub tool_calls: Vec<RecordedToolCall>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub new_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedToolCall {
    pub name: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub original_tokens: u64,
    #[serde(default)]
    pub delivered_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfidenceInterval {
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone)]
pub struct ArmSummary {
    pub successes: u64,
    pub sessions: u64,
    pub success_ci: ConfidenceInterval,
    pub mean_turns: f64,
    pub turns_ci: ConfidenceInterval,
    pub tool_calls: u64,
    pub expansions: u64,
    pub bounce_ci: ConfidenceInterval,
    pub total_cost_usd: f64,
    pub mean_cost_usd: f64,
    pub cost_ci: ConfidenceInterval,
    pub compression_saved_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub without: ArmSummary,
    pub with: ArmSummary,
    pub success_delta: ConfidenceInterval,
    pub mean_success_delta: f64,
    pub turn_delta: ConfidenceInterval,
    pub mean_turn_delta: f64,
    pub cost_delta: ConfidenceInterval,
    pub mean_cost_delta_usd: f64,
    pub total_savings_usd: f64,
}

#[derive(Default)]
struct ArmSamples {
    successes: u64,
    turns: Vec<f64>,
    costs: Vec<f64>,
    tool_calls: u64,
    saved_tokens: u64,
    bounce: BounceTracker,
}

pub fn load_replay(path: &Path) -> Result<ReplaySuite> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("reading replay metadata {}", path.display()))?;
    if metadata.len() > MAX_REPLAY_BYTES {
        bail!("replay exceeds {MAX_REPLAY_BYTES} byte limit");
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading replay {}", path.display()))?;
    let suite: ReplaySuite =
        serde_json::from_str(&raw).with_context(|| format!("parsing replay {}", path.display()))?;
    validate(&suite)?;
    Ok(suite)
}

fn validate(suite: &ReplaySuite) -> Result<()> {
    if suite.kind != REPLAY_KIND {
        bail!(
            "unsupported replay kind {:?}; expected {REPLAY_KIND}",
            suite.kind
        );
    }
    if suite.sessions.is_empty() {
        bail!("replay contains no sessions");
    }
    let mut ids = HashSet::new();
    for session in &suite.sessions {
        if session.id.trim().is_empty() || session.model.trim().is_empty() {
            bail!("session id and model must be non-empty");
        }
        if !ids.insert(&session.id) {
            bail!("duplicate session id {:?}", session.id);
        }
        for arm in [&session.without_compression, &session.with_compression] {
            if arm.turns == 0 {
                bail!("session {:?} has an arm with zero turns", session.id);
            }
            if arm
                .tool_calls
                .iter()
                .any(|call| call.name.trim().is_empty())
            {
                bail!("session {:?} has a tool call without a name", session.id);
            }
        }
    }
    Ok(())
}

pub fn replay(suite: &ReplaySuite) -> Result<ReplayReport> {
    validate(suite)?;
    let pricing = ModelPricing::embedded();
    let mut without = ArmSamples::default();
    let mut with = ArmSamples::default();
    let mut success_deltas = Vec::with_capacity(suite.sessions.len());
    let mut turn_deltas = Vec::with_capacity(suite.sessions.len());
    let mut cost_deltas = Vec::with_capacity(suite.sessions.len());

    let mut sessions: Vec<_> = suite.sessions.iter().collect();
    sessions.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    for session in sessions {
        let quote = pricing.quote(Some(&session.model));
        if quote.match_kind != PricingMatchKind::Exact {
            bail!(
                "session {:?} model {:?} has no exact embedded price",
                session.id,
                session.model
            );
        }
        let baseline_cost = record_arm(&mut without, &session.without_compression, quote.cost);
        let compressed_cost = record_arm(&mut with, &session.with_compression, quote.cost);
        success_deltas.push(
            f64::from(session.with_compression.success)
                - f64::from(session.without_compression.success),
        );
        turn_deltas
            .push(session.with_compression.turns as f64 - session.without_compression.turns as f64);
        cost_deltas.push(compressed_cost - baseline_cost);
    }

    let without = summarize(&without);
    let with = summarize(&with);
    Ok(ReplayReport {
        mean_success_delta: mean(&success_deltas),
        success_delta: mean_ci(&success_deltas),
        mean_turn_delta: mean(&turn_deltas),
        turn_delta: mean_ci(&turn_deltas),
        mean_cost_delta_usd: mean(&cost_deltas),
        cost_delta: mean_ci(&cost_deltas),
        total_savings_usd: without.total_cost_usd - with.total_cost_usd,
        without,
        with,
    })
}

fn record_arm(
    samples: &mut ArmSamples,
    arm: &RecordedArm,
    cost: crate::core::pricing::ModelCost,
) -> f64 {
    samples.successes += u64::from(arm.success);
    samples.turns.push(arm.turns as f64);
    let usd = cost.estimate_usd(
        arm.usage.new_input_tokens,
        arm.usage.output_tokens,
        arm.usage.cache_write_tokens,
        arm.usage.cache_read_tokens,
    );
    samples.costs.push(usd);
    for call in &arm.tool_calls {
        samples.tool_calls = samples.tool_calls.saturating_add(1);
        samples.saved_tokens = samples
            .saved_tokens
            .saturating_add(call.original_tokens.saturating_sub(call.delivered_tokens));

        samples.bounce.next_seq();
        if call.name == "ctx_expand" {
            samples.bounce.record_expansion(
                call.source.as_deref(),
                usize::try_from(call.delivered_tokens).unwrap_or(usize::MAX),
            );
        }
    }
    usd
}

fn summarize(samples: &ArmSamples) -> ArmSummary {
    let sessions = samples.turns.len() as u64;
    let tool_calls = samples.tool_calls;
    let expansions = samples.bounce.total_bounces();
    ArmSummary {
        successes: samples.successes,
        sessions,
        success_ci: wilson_ci(samples.successes, sessions),
        mean_turns: mean(&samples.turns),
        turns_ci: mean_ci(&samples.turns),
        tool_calls,
        expansions,
        bounce_ci: wilson_ci(expansions, tool_calls),
        total_cost_usd: samples.costs.iter().sum(),
        mean_cost_usd: mean(&samples.costs),
        cost_ci: mean_ci(&samples.costs),
        compression_saved_tokens: samples.saved_tokens,
    }
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn mean_ci(values: &[f64]) -> ConfidenceInterval {
    let avg = mean(values);
    if values.len() < 2 {
        return ConfidenceInterval {
            low: avg,
            high: avg,
        };
    }
    let variance =
        values.iter().map(|v| (v - avg).powi(2)).sum::<f64>() / (values.len() - 1) as f64;
    let margin = Z_95 * (variance / values.len() as f64).sqrt();
    ConfidenceInterval {
        low: avg - margin,
        high: avg + margin,
    }
}

fn wilson_ci(successes: u64, total: u64) -> ConfidenceInterval {
    if total == 0 {
        return ConfidenceInterval {
            low: 0.0,
            high: 0.0,
        };
    }
    let n = total as f64;
    let p = successes as f64 / n;
    let z2 = Z_95 * Z_95;
    let center = (p + z2 / (2.0 * n)) / (1.0 + z2 / n);
    let margin = Z_95 * ((p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt()) / (1.0 + z2 / n);
    ConfidenceInterval {
        low: (center - margin).max(0.0),
        high: (center + margin).min(1.0),
    }
}

pub fn format_markdown(report: &ReplayReport) -> String {
    let arm = |name: &str, a: &ArmSummary| {
        format!(
            "| {name} | {}/{} ({:.2}% [{:.2}, {:.2}]) | {:.2} [{:.2}, {:.2}] | {}/{} ({:.2}% [{:.2}, {:.2}]) | ${:.6} (${:.6} [{:.6}, {:.6}]) | {} |",
            a.successes,
            a.sessions,
            100.0 * a.successes as f64 / a.sessions as f64,
            100.0 * a.success_ci.low,
            100.0 * a.success_ci.high,
            a.mean_turns,
            a.turns_ci.low,
            a.turns_ci.high,
            a.expansions,
            a.tool_calls,
            if a.tool_calls == 0 {
                0.0
            } else {
                100.0 * a.expansions as f64 / a.tool_calls as f64
            },
            100.0 * a.bounce_ci.low,
            100.0 * a.bounce_ci.high,
            a.total_cost_usd,
            a.mean_cost_usd,
            a.cost_ci.low,
            a.cost_ci.high,
            a.compression_saved_tokens,
        )
    };
    let mut lines = vec![
        "# lean-ctx Compression Quality A/B Benchmark".to_string(),
        String::new(),
        "95% confidence intervals; paired deltas are `with - without`. Costs use the embedded ModelPricing snapshot and all four billed token classes.".to_string(),
        String::new(),
        "| Arm | Task success | Turns/session | ctx_expand / tool calls | Total cost (mean/session) | Tool tokens saved |".to_string(),
        "|---|---:|---:|---:|---:|---:|".to_string(),
        arm("Without compression", &report.without),
        arm("With compression", &report.with),
        String::new(),
        "## Paired impact".to_string(),
        String::new(),
        format!("- Success-rate delta: {:+.2} pp [{:+.2}, {:+.2}]", 100.0 * report.mean_success_delta, 100.0 * report.success_delta.low, 100.0 * report.success_delta.high),
        format!("- Turn delta per task: {:+.3} [{:+.3}, {:+.3}]", report.mean_turn_delta, report.turn_delta.low, report.turn_delta.high),
        format!("- Cost delta per task: {:+.6} USD [{:+.6}, {:+.6}]", report.mean_cost_delta_usd, report.cost_delta.low, report.cost_delta.high),
        format!("- Total dollar savings: ${:.6}", report.total_savings_usd),
    ];
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests;
