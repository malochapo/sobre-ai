use super::model::{Gotcha, GotchaStore};

/// A distilled learning from error-resolution correlation.
pub struct Learning {
    pub category: String,
    pub trigger: String,
    pub resolution: String,
    pub confidence: f32,
    pub occurrences: u32,
    pub sessions: usize,
}

impl std::fmt::Display for Learning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{cat}] {trigger} → {res} (confidence: {conf:.0}%, seen {occ}x across {sess} sessions)",
            cat = self.category,
            trigger = self.trigger,
            res = self.resolution,
            conf = self.confidence * 100.0,
            occ = self.occurrences,
            sess = self.sessions,
        )
    }
}

const MIN_CONFIDENCE: f32 = 0.5;
const MIN_OCCURRENCES: u32 = 2;

/// Extract high-confidence learnings from the gotcha store.
pub fn extract_learnings(store: &GotchaStore) -> Vec<Learning> {
    store
        .gotchas
        .iter()
        .filter(|g| g.confidence >= MIN_CONFIDENCE && g.occurrences >= MIN_OCCURRENCES)
        .map(gotcha_to_learning)
        .collect()
}

fn gotcha_to_learning(g: &Gotcha) -> Learning {
    Learning {
        category: g.category.short_label().to_string(),
        trigger: g.trigger.clone(),
        resolution: g.resolution.clone(),
        confidence: g.confidence,
        occurrences: g.occurrences,
        sessions: g.session_ids.len(),
    }
}
