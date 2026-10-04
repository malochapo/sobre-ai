use serde::Serialize;

const MAX_LADDER_STEPS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationVerdictV1 {
    Ok,
    Warn,
    Throttle,
    Block,
}

#[derive(Debug, Clone, Serialize)]
pub struct PressureSnapshotV1 {
    pub utilization_pct: u8,
    pub remaining_tokens: usize,
    pub action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DegradationDecisionV1 {
    pub verdict: DegradationVerdictV1,
    pub enforced: bool,
    pub throttle_ms: Option<u64>,
    pub reason_code: String,
    pub reason: String,
    pub ladder: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DegradationPolicyV1 {
    pub schema_version: u32,
    pub created_at: String,
    pub tool: String,
    pub budgets: crate::core::budget_tracker::BudgetSnapshot,
    pub slo: crate::core::slo::SloSnapshot,
    pub pressure: PressureSnapshotV1,
    pub decision: DegradationDecisionV1,
}

pub fn evaluate_v1_for_tool(tool: &str, created_at_override: Option<&str>) -> DegradationPolicyV1 {
    let created_at = created_at_override.map_or_else(
        || chrono::Utc::now().to_rfc3339(),
        std::string::ToString::to_string,
    );

    // SLO verdicts are advisory: they warn, they never block or throttle.
    let enforce = false;
    let throttle_ms = 250;

    let budgets = crate::core::budget_tracker::BudgetTracker::global().check();
    let slo = crate::core::slo::evaluate_quiet();
    let pressure = crate::core::context_ledger::ContextLedger::load().pressure();

    let pressure_snapshot = PressureSnapshotV1 {
        utilization_pct: (pressure.utilization * 100.0).min(254.0) as u8,
        remaining_tokens: pressure.remaining_tokens,
        action: format!("{:?}", pressure.recommendation),
    };

    let decision = decide(&budgets, &slo, &pressure, enforce, throttle_ms);

    DegradationPolicyV1 {
        schema_version: crate::core::contracts::DEGRADATION_POLICY_V1_SCHEMA_VERSION,
        created_at,
        tool: tool.to_string(),
        budgets,
        slo,
        pressure: pressure_snapshot,
        decision,
    }
}

fn decide(
    budgets: &crate::core::budget_tracker::BudgetSnapshot,
    slo: &crate::core::slo::SloSnapshot,
    pressure: &crate::core::context_ledger::ContextPressure,
    enforce: bool,
    throttle_ms: u64,
) -> DegradationDecisionV1 {
    use crate::core::budget_tracker::BudgetLevel;
    use crate::core::slo::SloAction;

    let mut ladder: Vec<String> = Vec::new();

    // Check correction-loop-induced degrade (Fix B: before budget/SLO checks)
    if crate::core::config::CompressionLevel::session_degrade_level().is_some() {
        ladder.push("compression_level=lite|off".to_string());
        return DegradationDecisionV1 {
            verdict: DegradationVerdictV1::Warn,
            enforced: false,
            throttle_ms: None,
            reason_code: "correction_rate_high".to_string(),
            reason:
                "correction_rate_high: compression auto-degraded due to repeated correction signals"
                    .to_string(),
            ladder,
        };
    }

    if *budgets.worst_level() == BudgetLevel::Exhausted {
        ladder.push("block".to_string());
        return DegradationDecisionV1 {
            verdict: DegradationVerdictV1::Block,
            enforced: true,
            throttle_ms: None,
            reason_code: "budget_exhausted".to_string(),
            reason: format!("budget_exhausted: {}", budgets.format_compact()),
            ladder,
        };
    }

    if slo.should_block() {
        ladder.push("reduce_scope".to_string());
        ladder.push("switch_mode=signatures".to_string());
        ladder.push("output_density=terse".to_string());
        ladder.push("block".to_string());
        return DegradationDecisionV1 {
            verdict: if enforce {
                DegradationVerdictV1::Block
            } else {
                DegradationVerdictV1::Warn
            },
            enforced: enforce,
            throttle_ms: None,
            reason_code: "slo_block".to_string(),
            reason: format!(
                "slo_block: worst_action={:?} violations={}",
                slo.worst_action,
                slo.violations
                    .iter()
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            ladder,
        };
    }

    if slo.should_throttle() {
        ladder.push("reduce_scope".to_string());
        ladder.push("switch_mode=map|signatures".to_string());
        ladder.push(format!("throttle_ms={throttle_ms}"));
        return DegradationDecisionV1 {
            verdict: if enforce {
                DegradationVerdictV1::Throttle
            } else {
                DegradationVerdictV1::Warn
            },
            enforced: enforce,
            throttle_ms: if enforce { Some(throttle_ms) } else { None },
            reason_code: "slo_throttle".to_string(),
            reason: format!(
                "slo_throttle: worst_action={:?} violations={}",
                slo.worst_action,
                slo.violations
                    .iter()
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            ladder,
        };
    }

    let budget_warn = *budgets.worst_level() == BudgetLevel::Warning;
    let pressure_warn = matches!(
        pressure.recommendation,
        crate::core::context_ledger::PressureAction::SuggestCompression
            | crate::core::context_ledger::PressureAction::ForceCompression
            | crate::core::context_ledger::PressureAction::EvictLeastRelevant
    );

    if budget_warn || pressure_warn || matches!(slo.worst_action, Some(SloAction::Warn)) {
        ladder.push("reduce_scope".to_string());
        ladder.push("switch_mode=signatures".to_string());
        ladder.push("output_density=compact|terse".to_string());
        ladder.truncate(MAX_LADDER_STEPS);
        return DegradationDecisionV1 {
            verdict: DegradationVerdictV1::Warn,
            enforced: false,
            throttle_ms: None,
            reason_code: "warn_only".to_string(),
            reason: "warn_only".to_string(),
            ladder,
        };
    }

    DegradationDecisionV1 {
        verdict: DegradationVerdictV1::Ok,
        enforced: false,
        throttle_ms: None,
        reason_code: "ok".to_string(),
        reason: "ok".to_string(),
        ladder: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::budget_tracker::{BudgetLevel, CostStatus, DimensionStatus};

    fn budget_snapshot(level: BudgetLevel) -> crate::core::budget_tracker::BudgetSnapshot {
        crate::core::budget_tracker::BudgetSnapshot {
            role: "coder".to_string(),
            tokens: DimensionStatus {
                used: 10,
                limit: 10,
                percent: 100,
                level: level.clone(),
            },
            shell: DimensionStatus {
                used: 0,
                limit: 0,
                percent: 0,
                level: BudgetLevel::Ok,
            },
            cost: CostStatus {
                used_usd: 0.0,
                limit_usd: 0.0,
                percent: 0,
                level,
            },
        }
    }

    #[test]
    fn budget_exhausted_blocks_even_when_enforce_false() {
        let b = budget_snapshot(BudgetLevel::Exhausted);
        let slo = crate::core::slo::SloSnapshot {
            slos: vec![],
            violations: vec![],
            worst_action: None,
        };
        let pressure = crate::core::context_ledger::ContextPressure {
            utilization: 0.1,
            remaining_tokens: 1000,
            entries_count: 0,
            recommendation: crate::core::context_ledger::PressureAction::NoAction,
        };
        let d = decide(&b, &slo, &pressure, false, 250);
        assert_eq!(d.verdict, DegradationVerdictV1::Block);
    }
}
