use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum MessagePriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl MessagePriority {
    pub fn parse_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "low" => Self::Low,
            "high" => Self::High,
            "critical" => Self::Critical,
            _ => Self::Normal,
        }
    }
}

impl std::fmt::Display for MessagePriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Normal => write!(f, "normal"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum PrivacyLevel {
    Public,
    #[default]
    Team,
    Private,
}

impl PrivacyLevel {
    pub fn parse_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "public" => Self::Public,
            "private" => Self::Private,
            _ => Self::Team,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AMessage {
    pub id: String,
    pub from_agent: String,
    pub to_agent: Option<String>,
    pub task_id: Option<String>,
    pub category: MessageCategory,
    pub priority: MessagePriority,
    pub privacy: PrivacyLevel,
    pub content: String,
    pub metadata: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub project_root: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub read_by: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageCategory {
    TaskDelegation,
    TaskUpdate,
    TaskResult,
    ContextShare,
    Question,
    Answer,
    Notification,
    Handoff,
}

impl MessageCategory {
    pub fn parse_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "task_delegation" | "delegation" => Self::TaskDelegation,
            "task_update" | "update" => Self::TaskUpdate,
            "task_result" | "result" => Self::TaskResult,
            "context_share" | "share" => Self::ContextShare,
            "question" => Self::Question,
            "answer" => Self::Answer,
            "handoff" => Self::Handoff,
            _ => Self::Notification,
        }
    }
}

impl std::fmt::Display for MessageCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TaskDelegation => write!(f, "task_delegation"),
            Self::TaskUpdate => write!(f, "task_update"),
            Self::TaskResult => write!(f, "task_result"),
            Self::ContextShare => write!(f, "context_share"),
            Self::Question => write!(f, "question"),
            Self::Answer => write!(f, "answer"),
            Self::Notification => write!(f, "notification"),
            Self::Handoff => write!(f, "handoff"),
        }
    }
}

impl A2AMessage {
    pub fn new(from: &str, to: Option<&str>, category: MessageCategory, content: &str) -> Self {
        Self {
            id: generate_msg_id(),
            from_agent: from.to_string(),
            to_agent: to.map(std::string::ToString::to_string),
            task_id: None,
            category,
            priority: MessagePriority::Normal,
            privacy: PrivacyLevel::Team,
            content: content.to_string(),
            metadata: std::collections::HashMap::new(),
            project_root: None,
            timestamp: Utc::now(),
            read_by: vec![from.to_string()],
            expires_at: None,
        }
    }

    pub fn with_task(mut self, task_id: &str) -> Self {
        self.task_id = Some(task_id.to_string());
        self
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| Utc::now() > exp)
    }
}

fn generate_msg_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand: u64 = (ts as u64).wrapping_mul(6364136223846793005);
    format!("msg-{:x}-{:08x}", ts % 0xFFFF_FFFF, rand as u32)
}
