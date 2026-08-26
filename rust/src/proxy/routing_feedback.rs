//! Adapter between proxy routing events and OCLA quality tracking.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::proxy::routing_quality::{RoutingDecision, RoutingOutcome, RoutingQualityTracker};

const MAX_PENDING_DECISIONS: usize = 1_000;
type PendingDecisions = HashMap<(String, String), VecDeque<RoutingDecision>>;

/// Collects proxy routing decisions and their measured outcomes.
#[derive(Clone, Debug)]
pub struct RoutingFeedback {
    tracker: Arc<Mutex<RoutingQualityTracker>>,
    pending_decisions: Arc<Mutex<PendingDecisions>>,
}

impl RoutingFeedback {
    /// Creates an empty routing feedback collector.
    pub fn new() -> Self {
        Self {
            tracker: Arc::new(Mutex::new(RoutingQualityTracker::new())),
            pending_decisions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Records a route selection until its measured outcome arrives.
    pub fn record_decision(&self, original: &str, routed: &str, reason: &str) {
        let decision = RoutingDecision {
            original_model: original.to_string(),
            routed_model: routed.to_string(),
            reason: reason.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        let key = (original.to_string(), routed.to_string());
        let mut pending = self
            .pending_decisions
            .lock()
            .expect("routing feedback pending decision mutex poisoned");
        let queue = pending.entry(key).or_default();
        queue.push_back(decision);
        while pending.values().map(VecDeque::len).sum::<usize>() > MAX_PENDING_DECISIONS {
            let Some(evicted_key) = pending
                .iter()
                .find(|(_, decisions)| !decisions.is_empty())
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(decisions) = pending.get_mut(&evicted_key) {
                decisions.pop_front();
                if decisions.is_empty() {
                    pending.remove(&evicted_key);
                }
            }
        }
    }

    /// Records measured quality for a route and forwards it to the tracker.
    pub fn record_outcome(
        &self,
        original: &str,
        routed: &str,
        quality: f64,
        tokens_saved: u64,
        latency_delta_ms: i64,
    ) {
        let key = (original.to_string(), routed.to_string());
        let decision = {
            let mut pending = self
                .pending_decisions
                .lock()
                .expect("routing feedback pending decision mutex poisoned");
            let decision = pending.get_mut(&key).and_then(VecDeque::pop_front);
            if pending.get(&key).is_some_and(VecDeque::is_empty) {
                pending.remove(&key);
            }
            decision
        };
        let decision = decision.unwrap_or_else(|| RoutingDecision {
            original_model: original.to_string(),
            routed_model: routed.to_string(),
            reason: "outcome without recorded decision".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

        self.tracker
            .lock()
            .expect("routing feedback tracker mutex poisoned")
            .record(RoutingOutcome {
                decision,
                quality_score: Some(quality),
                tokens_saved,
                latency_delta_ms,
            });
    }

    /// Returns whether tracked route quality warrants fallback.
    pub fn should_use_fallback(&self) -> bool {
        self.tracker
            .lock()
            .expect("routing feedback tracker mutex poisoned")
            .should_fallback()
    }

    /// Returns tracked success rate and average token savings.
    pub fn stats(&self) -> (f64, f64) {
        let tracker = self
            .tracker
            .lock()
            .expect("routing feedback tracker mutex poisoned");
        (tracker.success_rate(), tracker.average_savings())
    }
}

impl Default for RoutingFeedback {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_decision_until_matching_outcome() {
        let feedback = RoutingFeedback::new();

        feedback.record_decision("expensive", "fast", "token budget");
        assert_eq!(feedback.stats(), (0.0, 0.0));
        assert!(!feedback.should_use_fallback());

        let pending = feedback
            .pending_decisions
            .lock()
            .expect("test pending decision mutex poisoned");
        let key = (String::from("expensive"), String::from("fast"));
        let decision = &pending[&key][0];
        assert_eq!(decision.reason, "token budget");
    }

    #[test]
    fn records_successful_outcome_and_statistics() {
        let feedback = RoutingFeedback::new();

        feedback.record_decision("expensive", "fast", "token budget");
        feedback.record_outcome("expensive", "fast", 0.95, 120, -10);

        assert_eq!(feedback.stats(), (1.0, 120.0));
        assert!(!feedback.should_use_fallback());
    }

    #[test]
    fn poor_outcome_triggers_fallback() {
        let feedback = RoutingFeedback::new();

        feedback.record_outcome("expensive", "fast", 0.4, 20, 15);

        assert_eq!(feedback.stats(), (0.0, 20.0));
        assert!(feedback.should_use_fallback());
    }
}
