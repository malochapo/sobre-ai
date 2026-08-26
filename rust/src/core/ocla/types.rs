use std::cell::RefCell;

use serde::{Deserialize, Serialize};
use thiserror::Error;


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
}
