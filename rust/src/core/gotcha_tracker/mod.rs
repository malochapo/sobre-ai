//! The project's gotchas: pitfalls the agent records with
//! `sobre_knowledge action=gotcha`, which `sobre_compose` shows for a task
//! they match. Confidence decays with the time since a gotcha was last
//! recorded, and a gotcha that falls below the threshold is dropped.

mod model;
mod persist;

pub use model::{Gotcha, GotchaCategory, GotchaSeverity, GotchaStore};

use chrono::Utc;
pub(crate) use model::MAX_GOTCHAS;
use model::{DECAY_ARCHIVE_THRESHOLD, DECAY_PER_WEEK, gotcha_id};

impl GotchaStore {
    /// Records a gotcha the agent reported, or merges it into the one with
    /// the same trigger and category. `None` when the store is full of
    /// higher-confidence gotchas and this one was evicted.
    pub fn report_gotcha(
        &mut self,
        trigger: &str,
        resolution: &str,
        category: &str,
        severity: &str,
    ) -> Option<&Gotcha> {
        let cat = GotchaCategory::from_str_loose(category);
        let sev = match severity.to_lowercase().as_str() {
            "critical" => GotchaSeverity::Critical,
            "info" => GotchaSeverity::Info,
            _ => GotchaSeverity::Warning,
        };
        let id = gotcha_id(trigger, &cat);
        self.add_or_merge(Gotcha::new(cat, sev, trigger, resolution));
        self.updated_at = Utc::now();
        self.gotchas.iter().find(|g| g.id == id)
    }

    fn add_or_merge(&mut self, new: Gotcha) {
        if let Some(existing) = self.gotchas.iter_mut().find(|g| g.id == new.id) {
            existing.merge_with(&new);
        } else {
            self.gotchas.push(new);
            if self.gotchas.len() > MAX_GOTCHAS {
                self.gotchas.sort_by(|a, b| {
                    b.confidence
                        .partial_cmp(&a.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                self.gotchas.truncate(MAX_GOTCHAS);
            }
        }
    }

    /// Lowers each gotcha's confidence with the time since it was last seen,
    /// from its first day on, and drops the gotchas that fall below the
    /// threshold. Only the decay not taken off yet (since `decayed_at`) is
    /// applied, so applying it again later never counts the same days twice.
    pub fn apply_decay(&mut self) {
        let now = Utc::now();

        for gotcha in &mut self.gotchas {
            let age_at = |t: chrono::DateTime<Utc>| {
                (t - gotcha.last_seen).num_seconds().max(0) as f32 / 86_400.0
            };
            let age = age_at(now);
            if age < 1.0 {
                continue;
            }
            let already = gotcha
                .decayed_at
                .map(age_at)
                .filter(|a| *a >= 1.0)
                .unwrap_or(0.0);
            let occurrence_factor = 1.0 / (1.0 + gotcha.occurrences as f32 * 0.1);
            let decay = DECAY_PER_WEEK * occurrence_factor * ((age - already).max(0.0) / 7.0);
            gotcha.confidence = (gotcha.confidence - decay).max(0.0);
            gotcha.decayed_at = Some(now);
        }

        self.gotchas
            .retain(|g| g.confidence >= DECAY_ARCHIVE_THRESHOLD);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gotcha_store_add_and_merge() {
        let mut store = GotchaStore::new("testhash");
        let g1 = Gotcha::new(
            GotchaCategory::Build,
            GotchaSeverity::Critical,
            "error E0507",
            "use clone",
        );
        store.add_or_merge(g1);
        assert_eq!(store.gotchas.len(), 1);

        let g2 = Gotcha::new(
            GotchaCategory::Build,
            GotchaSeverity::Critical,
            "error E0507",
            "use ref pattern",
        );
        store.add_or_merge(g2);
        assert_eq!(store.gotchas.len(), 1);
        assert_eq!(store.gotchas[0].occurrences, 2);
    }

    #[test]
    fn agent_report_gotcha() {
        let mut store = GotchaStore::new("testhash");
        let g = store
            .report_gotcha(
                "Use thiserror not anyhow",
                "Derive thiserror::Error in library code",
                "convention",
                "warning",
            )
            .expect("gotcha should be retained in empty store");
        assert_eq!(g.confidence, 0.9);
        assert_eq!(g.category, GotchaCategory::Convention);
    }

    #[test]
    fn decay_reduces_confidence() {
        let mut store = GotchaStore::new("testhash");
        let mut g = Gotcha::new(
            GotchaCategory::Build,
            GotchaSeverity::Warning,
            "test error",
            "test fix",
        );
        g.last_seen = Utc::now() - chrono::Duration::days(30);
        g.confidence = 0.5;
        store.gotchas.push(g);

        store.apply_decay();
        assert!(store.gotchas[0].confidence < 0.5);
    }

    /// Loading the store applies decay, and every load is saved back: the
    /// decay a gotcha's age calls for is taken off once, not on every load.
    #[test]
    fn decay_is_applied_once_across_loads() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let root = "/tmp/sobre-gotcha-decay-once";
        let mut store = GotchaStore::new("h");
        let mut g = Gotcha::new(
            GotchaCategory::Build,
            GotchaSeverity::Warning,
            "old error",
            "old fix",
        );
        g.last_seen = Utc::now() - chrono::Duration::days(14);
        g.confidence = 0.9;
        store.gotchas.push(g);
        store.save(root).unwrap();

        let first = GotchaStore::load(root);
        let decayed = first.gotchas[0].confidence;
        assert!(decayed < 0.9);
        first.save(root).unwrap();
        let second = GotchaStore::load(root);
        assert!(
            (second.gotchas[0].confidence - decayed).abs() < 1e-4,
            "{} then {}",
            decayed,
            second.gotchas[0].confidence
        );
    }

    #[test]
    fn decay_archives_low_confidence() {
        let mut store = GotchaStore::new("testhash");
        let mut g = Gotcha::new(
            GotchaCategory::Build,
            GotchaSeverity::Info,
            "old error",
            "old fix",
        );
        g.last_seen = Utc::now() - chrono::Duration::days(90);
        g.confidence = 0.16;
        store.gotchas.push(g);

        store.apply_decay();
        assert!(store.gotchas.is_empty());
    }
}
