//! BuiltinAgentGateway — validates and relays agent-to-agent envelopes.
//!
//! Wraps `core/a2a/` behind the OCLA trait. Validates envelope fields,
//! emits AgentChainEvent to OclaBus, and returns the envelope with the
//! relay_id confirmed. Budget enforcement is checked but not consumed
//! (consumption happens at the transport layer).

use crate::core::a2a::message::{MessagePriority, PrivacyLevel};
use crate::core::agents::AgentRegistry;
use crate::core::ocla::capsule::global_capsule_store;
use crate::core::ocla::traits::{AgentGateway, OclaService};
use crate::core::ocla::types::{
    AgentEnvelope, OclaCapability, OclaCapabilityKind, OclaError, OclaResult,
};

pub struct BuiltinAgentGateway;

impl BuiltinAgentGateway {
    pub fn new() -> Self {
        Self
    }

    pub fn can_relay(&self, capsule_ref: &str, _to_agent_id: &str) -> bool {
        capsule_ref.is_empty() || global_capsule_store().resolve(capsule_ref).is_ok()
    }
    pub fn route_message(
        &self, // used for trait impl method grouping
        from_agent: &str,
        to_agent: Option<&str>,
        category: &str,
        message: &str,
        privacy: PrivacyLevel,
        priority: MessagePriority,
        ttl_hours: Option<u64>,
    ) -> OclaResult<String> {
        AgentRegistry::mutate_locked(|registry| {
            self.route_message_in_registry(
                registry, from_agent, to_agent, category, message, privacy, priority, ttl_hours,
            )
        })
        .map(|(_, message_id)| message_id)
        .map_err(|error| {
            OclaError::Rejected(
                OclaCapabilityKind::AgentGateway,
                format!("agent bus routing failed: {error}"),
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::unused_self)]
    fn route_message_in_registry(
        &self, // used for trait impl method grouping
        registry: &mut AgentRegistry,
        from_agent: &str,
        to_agent: Option<&str>,
        category: &str,
        message: &str,
        privacy: PrivacyLevel,
        priority: MessagePriority,
        ttl_hours: Option<u64>,
    ) -> String {
        registry.post_message_full(
            from_agent, to_agent, category, message, privacy, priority, ttl_hours,
        )
    }
}

impl Default for BuiltinAgentGateway {
    fn default() -> Self {
        Self::new()
    }
}

impl OclaService for BuiltinAgentGateway {
    fn capability(&self) -> OclaCapability {
        OclaCapability::available(OclaCapabilityKind::AgentGateway)
    }
}

impl AgentGateway for BuiltinAgentGateway {
    fn relay_agent(&self, envelope: AgentEnvelope) -> OclaResult<AgentEnvelope> {
        let mut envelope = envelope;
        if envelope.budget_tokens == 0 {
            return Err(OclaError::Rejected(
                OclaCapabilityKind::AgentGateway,
                "zero budget".into(),
            ));
        }
        if !envelope.capsule_ref.is_empty() {
            let parent_ref = envelope.capsule_ref.clone();
            envelope.capsule_ref = global_capsule_store()
                .fork(&parent_ref, envelope.budget_tokens)
                .map_err(|error| {
                    tracing::debug!(error = %error, "capsule fork failed for relay");
                    OclaError::Rejected(
                        OclaCapabilityKind::AgentGateway,
                        format!("capsule fork failed: {error}"),
                    )
                })?;
            tracing::debug!("capsule forked for relay");
        }


        Ok(envelope)
    }

    fn route_message(
        &self,
        from: &str,
        to: Option<&str>,
        category: &str,
        message: &str,
        privacy: PrivacyLevel,
        priority: MessagePriority,
        ttl_hours: Option<u64>,
    ) -> OclaResult<String> {
        BuiltinAgentGateway::route_message(
            self, from, to, category, message, privacy, priority, ttl_hours,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ocla::types::OclaRequestContext;

    fn envelope(budget: u64) -> AgentEnvelope {
        AgentEnvelope {
            schema_version: 1,
            relay_id: "relay:test".into(),
            context: OclaRequestContext {
                request_id: "r1".into(),
                session_id: "s1".into(),
                agent_id: "agent-test".into(),
                content_ref: "ref:test".into(),
                tenant_id: None,
                trace_id: "tr-unit".into(),
            },
            from_agent_id: "agent-a".into(),
            to_agent_id: "agent-b".into(),
            capsule_ref: String::new(),
            budget_tokens: budget,
        }
    }

    #[test]
    fn relay_without_capsule_passes_through() {
        let gateway = BuiltinAgentGateway::new();
        let result = gateway.relay_agent(envelope(1000)).unwrap();
        assert_eq!(result.from_agent_id, "agent-a");
        assert!(result.capsule_ref.is_empty());
    }

    #[test]
    fn relay_with_capsule_forks() {
        let gateway = BuiltinAgentGateway::new();
        let parent_ref = global_capsule_store().register(b"relay capsule");
        let mut input = envelope(1000);
        input.capsule_ref = parent_ref.clone();

        let result = gateway.relay_agent(input).expect("relay succeeds");

        assert_ne!(result.capsule_ref, parent_ref);
        assert_eq!(
            global_capsule_store()
                .resolve(&result.capsule_ref)
                .expect("child resolves"),
            b"relay capsule"
        );
    }

    #[test]
    fn can_relay_false_for_unknown_ref() {
        let gateway = BuiltinAgentGateway::new();

        assert!(gateway.can_relay("", "agent-b"));
        assert!(!gateway.can_relay("capsule:unknown-ref", "agent-b"));
    }

    #[test]
    fn relay_deducts_budget_tokens() {
        let gateway = BuiltinAgentGateway::new();
        let parent_ref = global_capsule_store().register(b"budget capsule");
        let mut input = envelope(321);
        input.capsule_ref = parent_ref;

        let result = gateway.relay_agent(input).expect("relay succeeds");

        assert_eq!(
            global_capsule_store()
                .budget_tokens(&result.capsule_ref)
                .expect("child budget exists"),
            321
        );
    }

    #[test]
    fn relay_rejects_zero_budget() {
        let gateway = BuiltinAgentGateway::new();
        assert!(gateway.relay_agent(envelope(0)).is_err());
    }

    #[test]
    fn route_message_writes_to_agent_bus() {
        let gateway = BuiltinAgentGateway::new();
        let mut registry = AgentRegistry::new();
        let message_id = gateway.route_message_in_registry(
            &mut registry,
            "agent-a",
            Some("agent-b"),
            "request",
            "Please review",
            PrivacyLevel::Private,
            MessagePriority::High,
            Some(2),
        );

        let messages = registry.read_unread("agent-b");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, message_id);
        assert_eq!(messages[0].message, "Please review");
        assert_eq!(messages[0].privacy, PrivacyLevel::Private);
        assert_eq!(messages[0].priority, MessagePriority::High);
    }

    #[test]
    fn route_message_supports_broadcast() {
        let gateway = BuiltinAgentGateway::new();
        let mut registry = AgentRegistry::new();
        gateway.route_message_in_registry(
            &mut registry,
            "agent-a",
            None,
            "status",
            "Ready",
            PrivacyLevel::Team,
            MessagePriority::Normal,
            None,
        );

        assert_eq!(registry.read_unread("agent-b").len(), 1);
    }

    #[test]
    fn registry_routes_message_through_agent_gateway() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        let registry = crate::core::ocla::registry::OclaRegistry::with_builtins();
        let message_id = registry
            .agent_gateway
            .route_message(
                "agent-a",
                Some("agent-b"),
                "request",
                "Please review",
                PrivacyLevel::Private,
                MessagePriority::High,
                Some(2),
            )
            .unwrap();

        assert!(!message_id.is_empty());
    }
}
