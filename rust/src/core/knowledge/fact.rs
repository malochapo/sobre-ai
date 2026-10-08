use chrono::{DateTime, Utc};

use super::types::KnowledgeFact;

impl KnowledgeFact {
    pub fn is_current(&self) -> bool {
        self.valid_until.is_none()
    }

    /// Stable, intrinsic quality metric (0.0..1.0).
    ///
    /// Based only on confidence, confirmation count, and feedback balance.
    /// Deliberately excludes volatile signals (retrieval count, recency) to
    /// keep recall output deterministic. For display ordering use
    /// `salience_score()` which adds recency and category weighting.
    pub fn quality_score(&self) -> f32 {
        let confidence = self.confidence.clamp(0.0, 1.0);
        let confirmations_norm = (self.confirmation_count.min(5) as f32) / 5.0;
        let balance = self.feedback_up as i32 - self.feedback_down as i32;
        let feedback_effect = (balance as f32 / 4.0).tanh() * 0.1;

        // IMPORTANT: quality_score must be stable across repeated recall calls.
        // Retrieval signals (retrieval_count/last_retrieved) are persisted, but should not change
        // the displayed "quality" score, otherwise recall output becomes non-deterministic.
        (0.8 * confidence + 0.2 * confirmations_norm + feedback_effect).clamp(0.0, 1.0)
    }

    pub fn was_valid_at(&self, at: DateTime<Utc>) -> bool {
        let after_start = self.valid_from.is_none_or(|from| at >= from);
        let before_end = self.valid_until.is_none_or(|until| at <= until);
        after_start && before_end
    }
}
