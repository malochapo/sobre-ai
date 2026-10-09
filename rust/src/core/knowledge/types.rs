use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::sensitivity::SensitivityLevel;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeArchetype {
    Pattern,
    Preference,
    Architecture,
    Gotcha,
    Convention,
    Dependency,
    Workflow,
    Observation,
    Decision,
    #[default]
    Fact,
}

impl KnowledgeArchetype {
    pub fn salience_bonus(&self) -> u32 {
        match self {
            Self::Architecture => 15,
            Self::Decision => 12,
            Self::Gotcha => 14,
            Self::Convention => 8,
            Self::Dependency => 6,
            Self::Pattern => 10,
            Self::Workflow => 7,
            Self::Preference => 5,
            Self::Observation => 3,
            Self::Fact => 0,
        }
    }

    /// Stable lowercase token used as the Open Knowledge Format (OKF) `type`
    /// field and in `sobre_archetype`. Single words, so snake_case == the
    /// serde name; kept as an explicit method to avoid a serde round-trip on
    /// the export hot path and to give a symmetric [`Self::from_type_str`].
    pub fn as_type_str(&self) -> &'static str {
        match self {
            Self::Pattern => "pattern",
            Self::Preference => "preference",
            Self::Architecture => "architecture",
            Self::Gotcha => "gotcha",
            Self::Convention => "convention",
            Self::Dependency => "dependency",
            Self::Workflow => "workflow",
            Self::Observation => "observation",
            Self::Decision => "decision",
            Self::Fact => "fact",
        }
    }

    /// Inverse of [`Self::as_type_str`]. Foreign OKF `type` values that sobre
    /// does not model fall back to [`Self::Fact`] — OKF only mandates `type`, so
    /// an unknown producer type is imported as a plain fact rather than rejected.
    pub fn from_type_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "pattern" => Self::Pattern,
            "preference" => Self::Preference,
            "architecture" => Self::Architecture,
            "gotcha" => Self::Gotcha,
            "convention" => Self::Convention,
            "dependency" => Self::Dependency,
            "workflow" => Self::Workflow,
            "observation" => Self::Observation,
            "decision" => Self::Decision,
            _ => Self::Fact,
        }
    }

    pub fn infer_from_category(category: &str) -> Self {
        match category.to_lowercase().as_str() {
            // `data_model`/`schema` are structural (provider-extracted); they join arch.
            "architecture" | "arch" | "data_model" | "schema" => Self::Architecture,
            "decision" | "decisions" => Self::Decision,
            // bugs/blockers are pitfalls → Gotcha.
            "gotcha" | "gotchas" | "known_bugs" | "known_issues" | "bug" | "bugs" | "blocker"
            | "blockers" => Self::Gotcha,
            "convention" | "conventions" | "style" => Self::Convention,
            "dependency" | "dependencies" | "deps" => Self::Dependency,
            "pattern" | "patterns" => Self::Pattern,
            "workflow" | "workflows" => Self::Workflow,
            "preference" | "preferences" | "pref" => Self::Preference,
            "observation" | "finding" | "findings" => Self::Observation,
            _ => Self::Fact,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectKnowledge {
    pub project_root: String,
    pub project_hash: String,
    pub facts: Vec<KnowledgeFact>,
    pub history: Vec<ConsolidatedInsight>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeFact {
    pub category: String,
    pub key: String,
    pub value: String,
    pub source_session: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub last_confirmed: DateTime<Utc>,
    pub retrieval_count: u32,
    pub last_retrieved: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub supersedes: Option<String>,
    pub confirmation_count: u32,
    pub feedback_up: u32,
    pub feedback_down: u32,
    pub last_feedback: Option<DateTime<Utc>>,
    /// Per-item sensitivity classification. Set at creation from
    /// content and enforced by the policy floor at injection time.
    pub sensitivity: SensitivityLevel,
    pub imported_from: Option<String>,
    pub archetype: KnowledgeArchetype,
    pub revision_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contradiction {
    pub existing_key: String,
    pub existing_value: String,
    pub new_value: String,
    pub category: String,
    pub severity: ContradictionSeverity,
    pub resolution: String,
}

/// Outcome of a write-time admission decision, returned by
/// `ProjectKnowledge::remember_admitted`. It is the difference between "the agent
/// asked to store X" and "what sobre actually persisted".
#[derive(Debug, Clone)]
pub enum AdmissionResult {
    /// Inserted / confirmed / superseded the normal way; carries any
    /// contradiction the write resolved.
    Stored(Option<Contradiction>),
    /// The value was a near-duplicate of an existing same-category fact under a
    /// different key and was merged into it (a confirmation bump) instead of
    /// growing the store. Carries the target's identity, its new confirmation
    /// count, and the value finally kept.
    Merged {
        category: String,
        key: String,
        confirmations: u32,
        value: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContradictionSeverity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedInsight {
    pub summary: String,
    pub from_sessions: Vec<String>,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod archetype_tests {
    use super::*;

    #[test]
    fn infer_archetype_from_category() {
        assert_eq!(
            KnowledgeArchetype::infer_from_category("architecture"),
            KnowledgeArchetype::Architecture
        );
        assert_eq!(
            KnowledgeArchetype::infer_from_category("gotcha"),
            KnowledgeArchetype::Gotcha
        );
        assert_eq!(
            KnowledgeArchetype::infer_from_category("random"),
            KnowledgeArchetype::Fact
        );
    }

    #[test]
    fn salience_bonus_ordering() {
        assert!(
            KnowledgeArchetype::Architecture.salience_bonus()
                > KnowledgeArchetype::Fact.salience_bonus()
        );
        assert!(
            KnowledgeArchetype::Gotcha.salience_bonus()
                > KnowledgeArchetype::Convention.salience_bonus()
        );
    }

    #[test]
    fn default_archetype_is_fact() {
        assert_eq!(KnowledgeArchetype::default(), KnowledgeArchetype::Fact);
    }
}
