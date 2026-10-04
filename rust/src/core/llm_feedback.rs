//! One LLM turn's feedback, fed to the adaptive mode policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFeedbackEvent {
    pub agent_id: String,
    pub intent: Option<String>,
    pub model: Option<String>,
    pub llm_input_tokens: u64,
    pub llm_output_tokens: u64,
    pub latency_ms: Option<u64>,
    pub note: Option<String>,
    pub ctx_read_last_mode: Option<String>,
    pub ctx_read_modes: Option<BTreeMap<String, u64>>,
    pub timestamp: String,
}
