//! One event of the context radar log (`context_radar.jsonl`), written by the
//! observe hook and read by the bypass hint and compaction sync.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadarEvent {
    pub ts: u64,
    pub event_type: String,
    pub tokens: usize,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
}
