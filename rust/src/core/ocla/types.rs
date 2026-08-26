use std::cell::RefCell;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const OCLA_API_VERSION: &str = "ocla/v1";
pub const CANONICAL_TOKEN_ENVELOPE_SCHEMA_VERSION: u16 = 1;
pub const AGENT_ENVELOPE_SCHEMA_VERSION: u16 = 1;

pub type OclaResult<T> = Result<T, OclaError>;

/// Stable identifiers required to join decisions across interception surfaces.
/// Payload bytes intentionally never belong in this contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OclaRequestContext {
    pub request_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub content_ref: String,
    pub tenant_id: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trace_id: String,
}

thread_local! {
    static CURRENT_REQUEST_CONTEXT: RefCell<Option<OclaRequestContext>> = const {
        RefCell::new(None)
    };
}

fn generate_trace_id() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("CSPRNG unavailable");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    );
    format!("tr-{uuid}")
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RequiredNullableString {
    Value(String),
    Null(()),
}

impl RequiredNullableString {
    fn into_option(self) -> Option<String> {
        match self {
            Self::Value(value) => Some(value),
            Self::Null(()) => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireContext {
    request_id: String,
    session_id: String,
    agent_id: String,
    content_ref: String,
    tenant_id: RequiredNullableString,
    #[serde(default)]
    trace_id: Option<String>,
}

impl<'de> Deserialize<'de> for OclaRequestContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WireContext::deserialize(deserializer)?;
        Ok(Self::new(
            wire.request_id,
            wire.session_id,
            wire.agent_id,
            wire.content_ref,
            wire.tenant_id.into_option(),
            wire.trace_id,
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenEnvelopeSurface {
    Mcp,
    Proxy,
    Shell,
    Agent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenFlowDirection {
    Input,
    Output,
}

/// Provider-neutral token accounting. Each field reflects a distinct lifecycle
/// stage and prevents a cache or delivery mechanism from being double-counted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBalanceV1 {
    pub original_tokens: u64,
    pub materialized_tokens: u64,
    pub delivered_tokens: u64,
    pub provider_billed_tokens: u64,
}

impl TokenBalanceV1 {
    pub fn validate(&self) -> OclaResult<()> {
        if self.materialized_tokens > self.original_tokens {
            return Err(OclaError::InvalidRequest(
                "materialized_tokens exceeds original_tokens".into(),
            ));
        }
        if self.delivered_tokens > self.materialized_tokens {
            return Err(OclaError::InvalidRequest(
                "delivered_tokens exceeds materialized_tokens".into(),
            ));
        }
        Ok(())
    }
}

/// Canonical, payload-free representation of a token decision at any engine
/// boundary. Provider adapters project into this type before ledger, policy or
/// external SDK code observes the request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalTokenEnvelopeV1 {
    pub schema_version: u16,
    pub context: OclaRequestContext,
    pub surface: TokenEnvelopeSurface,
    pub direction: TokenFlowDirection,
    pub provider: String,
    pub model: String,
    pub token_balance: TokenBalanceV1,
    pub route_ref: Option<String>,
    pub policy_ref: Option<String>,
    pub idempotency_key: String,
}

impl CanonicalTokenEnvelopeV1 {
    pub fn validate(&self) -> OclaResult<()> {
        if self.schema_version != CANONICAL_TOKEN_ENVELOPE_SCHEMA_VERSION {
            return Err(OclaError::UnsupportedVersion(
                self.schema_version.to_string(),
            ));
        }
        self.context.validate()?;
        self.token_balance.validate()?;
        for (label, value) in [
            ("provider", &self.provider),
            ("model", &self.model),
            ("idempotency_key", &self.idempotency_key),
        ] {
            if value.trim().is_empty() {
                return Err(OclaError::InvalidRequest(format!("{label} is required")));
            }
        }
        Ok(())
    }
}

impl OclaRequestContext {
    #[must_use]
    pub fn new(
        request_id: String,
        session_id: String,
        agent_id: String,
        content_ref: String,
        tenant_id: Option<String>,
        trace_id: Option<String>,
    ) -> Self {
        Self {
            request_id,
            session_id,
            agent_id,
            content_ref,
            tenant_id,
            trace_id: trace_id.unwrap_or_else(generate_trace_id),
        }
    }

    pub fn scope<R>(&self, operation: impl FnOnce() -> R) -> R {
        CURRENT_REQUEST_CONTEXT.with(|current| {
            let previous = current.replace(Some(self.clone()));
            let result = operation();
            current.replace(previous);
            result
        })
    }

    pub(crate) fn current_trace_id() -> Option<String> {
        CURRENT_REQUEST_CONTEXT.with(|current| {
            current
                .borrow()
                .as_ref()
                .map(|context| context.trace_id.clone())
        })
    }

    pub fn validate(&self) -> OclaResult<()> {
        for (label, value) in [
            ("request_id", &self.request_id),
            ("session_id", &self.session_id),
            ("agent_id", &self.agent_id),
            ("content_ref", &self.content_ref),
            ("trace_id", &self.trace_id),
        ] {
            if value.trim().is_empty() {
                return Err(OclaError::InvalidRequest(format!("{label} is required")));
            }
        }
        Ok(())
    }
}

/// Canonical, payload-free admission contract for one A2A relay.
///
/// `budget_tokens` is an authorization ceiling, never observed delivery or
/// savings evidence. A transport must create its own measured token envelope
/// only after it actually materializes and delivers the handoff.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEnvelope {
    pub schema_version: u16,
    /// Content-derived relay identity for idempotent admission and event joins.
    pub relay_id: String,
    pub context: OclaRequestContext,
    pub from_agent_id: String,
    pub to_agent_id: String,
    pub capsule_ref: String,
    pub budget_tokens: u64,
}

impl AgentEnvelope {
    /// Assigns the deterministic identity after all relay fields are set.
    pub fn assign_relay_id(&mut self) -> OclaResult<()> {
        self.relay_id = self.computed_relay_id()?;
        Ok(())
    }

    /// Derives a stable relay ID without including payload bytes or the ID itself.
    pub fn computed_relay_id(&self) -> OclaResult<String> {
        let mut canonical = self.clone();
        canonical.relay_id = "agent-relay:pending".to_string();
        let bytes = serde_json::to_vec(&canonical).map_err(|error| {
            OclaError::InvalidRequest(format!("cannot serialize agent relay: {error}"))
        })?;
        Ok(format!("agent-relay:{}", blake3::hash(&bytes).to_hex()))
    }

    pub fn validate(&self) -> OclaResult<()> {
        if self.schema_version != AGENT_ENVELOPE_SCHEMA_VERSION {
            return Err(OclaError::UnsupportedVersion(
                self.schema_version.to_string(),
            ));
        }
        self.context.validate()?;
        for (label, value) in [
            ("from_agent_id", &self.from_agent_id),
            ("to_agent_id", &self.to_agent_id),
        ] {
            valid_agent_id(value)
                .then_some(())
                .ok_or_else(|| OclaError::InvalidRequest(format!("invalid {label}")))?;
        }
        if self.context.agent_id != self.from_agent_id {
            return Err(OclaError::InvalidRequest(
                "context agent_id must match from_agent_id".to_string(),
            ));
        }
        valid_digest_ref("capsule", "capsule:", &self.capsule_ref)?;
        valid_digest_ref("relay", "agent-relay:", &self.relay_id)?;
        if self.budget_tokens == 0 {
            return Err(OclaError::InvalidRequest(
                "agent relay budget_tokens must be greater than zero".to_string(),
            ));
        }
        if self.relay_id != self.computed_relay_id()? {
            return Err(OclaError::InvalidRequest(
                "agent relay_id does not match canonical relay content".to_string(),
            ));
        }
        Ok(())
    }
}

fn valid_agent_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_digest_ref(label: &str, prefix: &str, value: &str) -> OclaResult<()> {
    let digest = value.strip_prefix(prefix).ok_or_else(|| {
        OclaError::InvalidRequest(format!("{label}_ref must use {prefix}BLAKE3-hex form"))
    })?;
    (digest.len() == 64
        && digest.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
        }))
    .then_some(())
    .ok_or_else(|| OclaError::InvalidRequest(format!("invalid {label}_ref")))
}

#[derive(Debug, Error)]
pub enum OclaError {
    #[error("invalid OCLA request: {0}")]
    InvalidRequest(String),
    #[error("unsupported OCLA contract version: {0}")]
    UnsupportedVersion(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_context_rejects_incomplete_lineage() {
        let context = OclaRequestContext {
            request_id: "request".into(),
            session_id: String::new(),
            agent_id: "agent".into(),
            content_ref: "blake3:content".into(),
            tenant_id: None,
            trace_id: "tr-test".into(),
        };
        assert!(matches!(
            context.validate(),
            Err(OclaError::InvalidRequest(_))
        ));
    }

    #[test]
    fn wire_context_requires_an_explicit_nullable_tenant_id() {
        let missing = r#"{
            "request_id":"request",
            "session_id":"session",
            "agent_id":"agent",
            "content_ref":"blake3:content"
        }"#;
        assert!(serde_json::from_str::<WireContext>(missing).is_err());

        let explicit_null = r#"{
            "request_id":"request",
            "session_id":"session",
            "agent_id":"agent",
            "content_ref":"blake3:content",
            "tenant_id":null
        }"#;
        let context = serde_json::from_str::<WireContext>(explicit_null).expect("explicit null");
        assert!(matches!(
            context.tenant_id,
            RequiredNullableString::Null(())
        ));

        let explicit_value = r#"{
            "request_id":"request",
            "session_id":"session",
            "agent_id":"agent",
            "content_ref":"blake3:content",
            "tenant_id":"tenant"
        }"#;
        let context = serde_json::from_str::<WireContext>(explicit_value).expect("tenant string");
        assert!(matches!(
            context.tenant_id,
            RequiredNullableString::Value(ref value) if value == "tenant"
        ));

        let wrong_type = r#"{
            "request_id":"request",
            "session_id":"session",
            "agent_id":"agent",
            "content_ref":"blake3:content",
            "tenant_id":42
        }"#;
        assert!(serde_json::from_str::<WireContext>(wrong_type).is_err());
    }

    #[test]
    fn request_context_generates_or_preserves_trace_id() {
        let generated = OclaRequestContext::new(
            "request".into(),
            "session".into(),
            "agent".into(),
            "blake3:content".into(),
            None,
            None,
        );
        assert!(generated.trace_id.starts_with("tr-"));
        assert_eq!(generated.trace_id.len(), 39);

        let mut provided = serde_json::json!({
            "request_id": "request",
            "session_id": "session",
            "agent_id": "agent",
            "content_ref": "blake3:content",
            "tenant_id": null
        });
        provided["trace_id"] = serde_json::Value::String("tr-provided".into());
        let preserved: OclaRequestContext =
            serde_json::from_value(provided).expect("context preserves trace");
        assert_eq!(preserved.trace_id, "tr-provided");
    }

    #[test]
    fn agent_envelope_is_canonical_and_rejects_lineage_or_budget_drift() {
        let mut envelope = AgentEnvelope {
            schema_version: AGENT_ENVELOPE_SCHEMA_VERSION,
            relay_id: "agent-relay:pending".to_string(),
            context: OclaRequestContext {
                request_id: "request".into(),
                session_id: "session".into(),
                agent_id: "owner-agent".into(),
                content_ref: "blake3:content".into(),
                tenant_id: None,
                trace_id: "tr-test".into(),
            },
            from_agent_id: "owner-agent".into(),
            to_agent_id: "reviewer-agent".into(),
            capsule_ref: format!("capsule:{}", "a".repeat(64)),
            budget_tokens: 900,
        };
        envelope.assign_relay_id().expect("relay identity assigns");
        envelope.validate().expect("canonical relay validates");

        let mut wire = serde_json::to_value(&envelope).expect("relay serializes");
        wire.as_object_mut()
            .expect("relay is an object")
            .insert("unexpected".to_string(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<AgentEnvelope>(wire).is_err());

        envelope.budget_tokens = 0;
        assert!(matches!(
            envelope.validate(),
            Err(OclaError::InvalidRequest(_))
        ));
    }
}
