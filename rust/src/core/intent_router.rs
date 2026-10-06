//! Read mode recommended for a model tier and task type.

use crate::core::intent_engine::{ModelTier, TaskType};

pub fn read_mode_for_tier(tier: ModelTier, task_type: TaskType) -> String {
    // Editing tasks need the real, complete file — never an abbreviated,
    // signature-only, or identifier-obfuscated view — otherwise the agent is
    // forced into follow-up re-reads mid-edit. This holds across every tier
    // (a Fast-tier bugfix still has to see the code it changes).
    if matches!(
        task_type,
        TaskType::Refactor | TaskType::FixBug | TaskType::Generate
    ) {
        return "full".to_string();
    }
    match (tier, task_type) {
        (ModelTier::Fast, _) => "signatures".to_string(),
        (ModelTier::Standard, TaskType::Explore | TaskType::Review) => "map".to_string(),
        (ModelTier::Standard, _) => "full".to_string(),
        (ModelTier::Premium, _) => "auto".to_string(),
    }
}
