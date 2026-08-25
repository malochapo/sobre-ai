//! BuiltinModelRouter — intent-aware model routing via OCLA trait.
//!
//! Wraps `proxy/model_router.rs` and `proxy/effort_routing.rs` behind the
//! canonical trait. Emits ModelRouted events. Routes to the best candidate
//! model within the cost/latency constraints.

use crate::core::config::{Config, Effort, RoutingRules, parse_route_target};
use crate::core::ocla::traits::{ModelRouter, OclaService};
use crate::core::ocla::types::{
    IntentDecision, ModelRouteRequest, OclaCapability, OclaCapabilityKind, OclaResult,
    RoutingDecision,
};
use crate::core::savings_ledger::store::{self, MechanismSummary};
use serde_json::json;

pub struct BuiltinModelRouter {
    rules: RoutingRules,
}

impl BuiltinModelRouter {
    pub fn new() -> Self {
        Self::with_rules(Config::load().proxy.routing)
    }

    pub(crate) fn with_rules(rules: RoutingRules) -> Self {
        Self { rules }
    }
}

impl Default for BuiltinModelRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl OclaService for BuiltinModelRouter {
    fn capability(&self) -> OclaCapability {
        OclaCapability::available(OclaCapabilityKind::ModelRouter)
    }
}

/// Routing result with deterministic rationale for OCLA observability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingDecisionWithRationale {
    pub decision: RoutingDecision,
    pub routing_rationale: String,
}

impl ModelRouter for BuiltinModelRouter {
    fn route_model(&self, request: ModelRouteRequest) -> OclaResult<RoutingDecision> {
        Ok(self.route_model_with_intent(&request, None)?.decision)
    }
}

impl BuiltinModelRouter {
    /// Routes using a classifier decision when available, while preserving the
    /// proxy router as the fallback for absent or low-confidence decisions.
    pub fn route_model_with_intent(
        &self,
        request: &ModelRouteRequest,
        intent: Option<&IntentDecision>,
    ) -> OclaResult<RoutingDecisionWithRationale> {
        let requested_model = request
            .candidate_models
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string());
        let body = json!({
            "model": requested_model.clone(),
            "messages": [{"role": "user", "content": request.context.content_ref}]
        });
        let routed = intent
            .as_ref()
            .and_then(|decision| route_for_intent(&requested_model, decision, &self.rules))
            .or_else(|| crate::proxy::model_router::route(&body, &self.rules));
        let ledger = routing_ledger_summary();
        let (model, provider, tier, _model_changed) = routed.map_or_else(
            || {
                (
                    requested_model.clone(),
                    infer_provider(&requested_model),
                    "standard".to_string(),
                    false,
                )
            },
            |decision| {
                let model = decision.routed_model;
                let provider = decision
                    .routed_provider
                    .unwrap_or_else(|| infer_provider(&model));
                let changed = decision.model_changed;
                (model, provider, decision.tier, changed)
            },
        );


        let routing_rationale = routing_rationale(intent, tier.as_str(), ledger.as_ref());
        Ok(RoutingDecisionWithRationale {
            decision: RoutingDecision {
                model,
                provider,
                reasoning_budget_tokens: configured_reasoning_budget(),
                decision_ref: format!("route:{}", request.context.request_id),
            },
            routing_rationale,
        })
    }
}

fn route_for_intent(
    requested_model: &str,
    decision: &IntentDecision,
    rules: &RoutingRules,
) -> Option<crate::proxy::model_router::RoutingDecision> {
    if decision.confidence_milli < 500
        || !rules.is_active()
        || rules.tiers.is_empty()
        || rules.aliases.contains_key(requested_model)
    {
        return None;
    }
    let tier = intent_tier(&decision.intent);
    let target = rules.tiers.get(tier)?;
    if target.is_empty() {
        return Some(crate::proxy::model_router::RoutingDecision {
            requested_model: requested_model.to_string(),
            routed_model: requested_model.to_string(),
            routed_provider: None,
            tier: tier.to_string(),
            confidence: f64::from(decision.confidence_milli) / 1000.0,
            reasoning: format!("classifier intent selected {tier} tier"),
            model_changed: false,
            estimated_cost_ratio: None,
        });
    }
    let (provider, model) = parse_route_target(target)?;
    Some(crate::proxy::model_router::RoutingDecision {
        requested_model: requested_model.to_string(),
        routed_model: model.to_string(),
        routed_provider: provider.map(str::to_string),
        tier: tier.to_string(),
        confidence: f64::from(decision.confidence_milli) / 1000.0,
        reasoning: format!("classifier intent selected {tier} tier"),
        model_changed: requested_model != model,
        estimated_cost_ratio: None,
    })
}

fn intent_tier(intent: &str) -> &'static str {
    let intent = intent.to_ascii_lowercase();
    if [
        "read",
        "list",
        "show",
        "explain",
        "summarize",
        "status",
        "search",
        "lookup",
    ]
    .iter()
    .any(|term| intent.contains(term))
    {
        "fast"
    } else if [
        "code",
        "fix",
        "implement",
        "refactor",
        "debug",
        "build",
        "patch",
        "test",
    ]
    .iter()
    .any(|term| intent.contains(term))
    {
        "premium"
    } else {
        "standard"
    }
}

fn routing_ledger_summary() -> Option<MechanismSummary> {
    let path = store::default_path()?;
    store::summarize_by_mechanism(&path).remove("routing")
}

fn routing_rationale(
    intent: Option<&IntentDecision>,
    tier: &str,
    ledger: Option<&MechanismSummary>,
) -> String {
    let intent_part = intent.map_or_else(
        || "proxy classifier".to_string(),
        |decision| {
            format!(
                "classifier intent '{}' (confidence {})",
                decision.intent, decision.confidence_milli
            )
        },
    );
    let ledger_part = match ledger {
        Some(summary) if summary.saved_usd > 0.0 => format!(
            "ledger confirms {} routing events saving ${:.6}",
            summary.count, summary.saved_usd
        ),
        Some(summary) => format!(
            "ledger reports {} routing events without positive savings",
            summary.count
        ),
        None => "routing savings history unavailable".to_string(),
    };
    format!("{intent_part} selected {tier} tier; {ledger_part}")
}

fn configured_reasoning_budget() -> u64 {
    reasoning_budget(Config::load().proxy.resolved_effort())
}

fn reasoning_budget(effort: Option<Effort>) -> u64 {
    match effort {
        Some(Effort::Minimal) => 1_024,
        Some(Effort::Low) => 2_048,
        Some(Effort::Medium) => 4_096,
        Some(Effort::High) => 8_192,
        None => 0,
    }
}

fn infer_provider(model: &str) -> String {
    if model.contains("gpt") || model.contains("o1") || model.contains("o3") {
        "openai".to_string()
    } else if model.contains("claude") {
        "anthropic".to_string()
    } else if model.contains("gemini") {
        "google".to_string()
    } else {
        "unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ocla::types::OclaRequestContext;
    use std::collections::BTreeMap;

    fn route_req(candidates: &[&str]) -> ModelRouteRequest {
        ModelRouteRequest {
            context: OclaRequestContext {
                request_id: "r1".into(),
                session_id: "s1".into(),
                agent_id: "agent-test".into(),
                content_ref: "ref:test".into(),
                tenant_id: None,
                trace_id: "tr-unit".into(),
            },
            candidate_models: candidates.iter().map(|s| (*s).to_string()).collect(),
            maximum_cost_micros: None,
            maximum_latency_ms: None,
        }
    }

    fn active_rules(tiers: &[(&str, &str)]) -> RoutingRules {
        RoutingRules {
            enabled: Some(true),
            aliases: BTreeMap::new(),
            tiers: tiers
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    #[test]
    fn routes_first_candidate() {
        let router = BuiltinModelRouter::new();
        let decision = router
            .route_model(route_req(&["gpt-4o", "claude-3"]))
            .unwrap();
        assert_eq!(decision.model, "gpt-4o");
        assert_eq!(decision.provider, "openai");
    }

    #[test]
    fn infers_anthropic_provider() {
        let router = BuiltinModelRouter::new();
        let decision = router.route_model(route_req(&["claude-sonnet-4"])).unwrap();
        assert_eq!(decision.provider, "anthropic");
    }

    #[test]
    fn delegates_tier_selection_to_proxy_router() {
        let router =
            BuiltinModelRouter::with_rules(active_rules(&[("fast", "anthropic:claude-haiku-4-5")]));
        let mut request = route_req(&["claude-sonnet-4"]);
        request.context.content_ref = "explain how the cache works".into();

        let decision = router.route_model(request).unwrap();

        assert_eq!(decision.model, "claude-haiku-4-5");
        assert_eq!(decision.provider, "anthropic");
    }

    #[test]
    fn unknown_model_falls_back_to_default() {
        let router = BuiltinModelRouter::new();
        let decision = router.route_model(route_req(&[])).unwrap();
        assert_eq!(decision.model, "default");
        assert_eq!(decision.provider, "unknown");
        assert_eq!(decision.decision_ref, "route:r1");
    }

    #[test]
    fn registry_path_routes_with_configured_budget() {
        let registry = crate::core::ocla::registry::OclaRegistry::with_builtins();
        let decision = registry
            .model_router
            .route_model(route_req(&["gpt-4o"]))
            .unwrap();

        assert_eq!(decision.model, "gpt-4o");
        assert_eq!(
            decision.reasoning_budget_tokens,
            configured_reasoning_budget()
        );
    }

    #[test]
    fn maps_configured_effort_to_budget() {
        assert_eq!(reasoning_budget(Some(Effort::Minimal)), 1_024);
        assert_eq!(reasoning_budget(Some(Effort::High)), 8_192);
        assert_eq!(reasoning_budget(None), 0);
    }

    #[test]
    fn classifier_routes_simple_intent_to_fast_tier() {
        let router = BuiltinModelRouter::with_rules(active_rules(&[
            ("fast", "anthropic:claude-haiku-4-5"),
            ("premium", "openai:gpt-5"),
        ]));
        let result = router
            .route_model_with_intent(
                &route_req(&["gpt-4o"]),
                Some(&IntentDecision {
                    intent: "read the config".into(),
                    confidence_milli: 900,
                    rationale_ref: None,
                }),
            )
            .unwrap();

        assert_eq!(result.decision.model, "claude-haiku-4-5");
        assert!(result.routing_rationale.contains("read the config"));
        assert!(result.routing_rationale.contains("fast tier"));
    }

    #[test]
    fn ledger_rationale_distinguishes_material_savings() {
        let summary = MechanismSummary {
            count: 2,
            saved_tokens: 0,
            saved_usd: 0.25,
        };
        let rationale = routing_rationale(None, "fast", Some(&summary));
        assert!(rationale.contains("confirms 2 routing events"));
        assert!(rationale.contains("$0.250000"));
    }
}
