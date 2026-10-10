use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_GOTCHAS: usize = 100;
pub(super) const DECAY_ARCHIVE_THRESHOLD: f32 = 0.15;
/// Confidence a gotcha starts with when the agent records it.
pub(super) const REPORTED_CONFIDENCE: f32 = 0.9;
/// Confidence lost per week since a gotcha was last recorded, before the
/// occurrence factor.
pub(super) const DECAY_PER_WEEK: f32 = 0.02;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GotchaCategory {
    Build,
    Test,
    Config,
    Runtime,
    Dependency,
    Platform,
    Convention,
    Security,
}

impl GotchaCategory {
    pub fn from_str_loose(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "build" | "compile" => Self::Build,
            "test" => Self::Test,
            "config" | "configuration" => Self::Config,
            "runtime" => Self::Runtime,
            "dependency" | "dep" | "deps" => Self::Dependency,
            "platform" | "os" => Self::Platform,
            "security" | "sec" => Self::Security,
            _ => Self::Convention,
        }
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
            Self::Config => "config",
            Self::Runtime => "runtime",
            Self::Dependency => "dep",
            Self::Platform => "platform",
            Self::Convention => "conv",
            Self::Security => "sec",
        }
    }
}

impl std::fmt::Display for GotchaCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.short_label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GotchaSeverity {
    Critical,
    Warning,
    Info,
}

impl GotchaSeverity {
    pub fn multiplier(&self) -> f32 {
        match self {
            Self::Critical => 1.5,
            Self::Warning => 1.0,
            Self::Info => 0.7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gotcha {
    pub id: String,
    pub category: GotchaCategory,
    pub severity: GotchaSeverity,
    pub trigger: String,
    pub resolution: String,
    pub occurrences: u32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub confidence: f32,
    /// When decay was last taken off `confidence`, so a store loaded and
    /// saved again is not decayed twice for the same time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decayed_at: Option<DateTime<Utc>>,
}

impl Gotcha {
    pub fn new(
        category: GotchaCategory,
        severity: GotchaSeverity,
        trigger: &str,
        resolution: &str,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: gotcha_id(trigger, &category),
            category,
            severity,
            trigger: trigger.to_string(),
            resolution: resolution.to_string(),
            occurrences: 1,
            first_seen: now,
            last_seen: now,
            confidence: REPORTED_CONFIDENCE,
            decayed_at: None,
        }
    }

    pub fn merge_with(&mut self, other: &Gotcha) {
        self.occurrences += other.occurrences;
        if other.last_seen > self.last_seen {
            self.last_seen = other.last_seen;
            self.resolution.clone_from(&other.resolution);
        }
        self.confidence = self.confidence.max(other.confidence);
    }
}

// ---------------------------------------------------------------------------
// GotchaStore
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GotchaStore {
    pub project_hash: String,
    pub gotchas: Vec<Gotcha>,
    pub updated_at: DateTime<Utc>,
}

impl GotchaStore {
    pub fn new(project_hash: &str) -> Self {
        Self {
            project_hash: project_hash.to_string(),
            gotchas: Vec::new(),
            updated_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A gotcha's id, stored with it and matched again later: a stable hash,
/// the same with any Rust toolchain.
pub(super) fn gotcha_id(trigger: &str, category: &GotchaCategory) -> String {
    crate::core::hasher::hash_short(&format!("{trigger}\0{}", category.short_label()))
}
