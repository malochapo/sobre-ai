use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanditArm {
    pub name: String,
    pub alpha: f64,
    pub beta: f64,
    pub entropy_threshold: f64,
    pub jaccard_threshold: f64,
    pub budget_ratio: f64,
}

impl BanditArm {
    pub fn update_from_feedback(&mut self, outcome: &crate::core::feedback::CompressionOutcome) {
        let efficiency = if outcome.tokens_original > 0 {
            outcome.tokens_saved as f64 / outcome.tokens_original as f64
        } else {
            0.0
        };
        let success = efficiency > 0.3 && outcome.task_completed;
        if success {
            self.update_success();
        } else {
            self.update_failure();
        }
    }

    pub fn update_success(&mut self) {
        self.alpha += 1.0;
    }

    pub fn update_failure(&mut self) {
        self.beta += 1.0;
    }

    pub fn decay(&mut self, factor: f64) {
        self.alpha = (self.alpha * factor).max(1.0);
        self.beta = (self.beta * factor).max(1.0);
    }

    pub fn mean(&self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdBandit {
    pub arms: Vec<BanditArm>,
    pub total_pulls: u64,
}

impl Default for ThresholdBandit {
    fn default() -> Self {
        Self {
            arms: vec![
                BanditArm {
                    name: "conservative".to_string(),
                    alpha: 2.0,
                    beta: 1.0,
                    entropy_threshold: 1.2,
                    jaccard_threshold: 0.8,
                    budget_ratio: 0.5,
                },
                BanditArm {
                    name: "balanced".to_string(),
                    alpha: 2.0,
                    beta: 1.0,
                    entropy_threshold: 0.9,
                    jaccard_threshold: 0.7,
                    budget_ratio: 0.35,
                },
                BanditArm {
                    name: "aggressive".to_string(),
                    alpha: 2.0,
                    beta: 1.0,
                    entropy_threshold: 0.6,
                    jaccard_threshold: 0.55,
                    budget_ratio: 0.2,
                },
            ],
            total_pulls: 0,
        }
    }
}

impl ThresholdBandit {
    /// Choose an arm for this decision. Deterministic by default — the arm with
    /// the highest posterior mean (argmax) — so tool behavior is reproducible and
    /// provider prompt caching stays valid (#498). Probabilistic exploration
    /// (epsilon-greedy + Thompson sampling) is used only when
    /// [`Config::is_stochastic_enabled`] is on (auto-mode-learning). Always counts the pull and registers activity (#4).
    ///
    /// [`Config::is_stochastic_enabled`]: crate::core::config::Config::is_stochastic_enabled
    pub fn choose_arm(&mut self) -> &BanditArm {
        self.total_pulls += 1;
        crate::core::introspect::tick("field_weights_bandit");
        let idx = self.best_arm_idx_by_mean();
        &self.arms[idx]
    }

    /// Deterministic argmax of the posterior mean. Tie-break by lowest index so
    /// the choice is stable and reproducible.
    pub fn best_arm_idx_by_mean(&self) -> usize {
        self.arms
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.mean()
                    .partial_cmp(&b.mean())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(i, _)| i)
    }

    pub fn update(&mut self, arm_name: &str, success: bool) {
        if let Some(arm) = self.arms.iter_mut().find(|a| a.name == arm_name) {
            if success {
                arm.update_success();
            } else {
                arm.update_failure();
            }
        }
    }

    pub fn decay_all(&mut self, factor: f64) {
        for arm in &mut self.arms {
            arm.decay(factor);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BanditStore {
    pub bandits: HashMap<String, ThresholdBandit>,
}

/// Unified key format: `{domain}:{ext}:{bucket}`.
///
/// Separates three previously conflated BanditStore consumers:
/// - `feedback:{ext}` — compression quality per language (was `{ext}_feedback`)
/// - `threshold:{ext}:{sm|md|lg|xl}` — adaptive threshold tuning (was `{ext}_{bucket}`)
/// - `mode:{ext}:{sm|md|lg|xl}` — auto mode selection (was `{ext}_{bucket}`, now separate)
pub fn bandit_key(domain: &str, ext: &str, bucket: Option<&str>) -> String {
    match bucket {
        Some(b) => format!("{domain}:{ext}:{b}"),
        None => format!("{domain}:{ext}"),
    }
}

fn split_legacy_sized_key(key: &str) -> Option<(&str, &str)> {
    for suffix in &["_sm", "_md", "_lg", "_xl"] {
        if let Some(ext) = key.strip_suffix(suffix) {
            return Some((ext, &suffix[1..]));
        }
    }
    None
}

impl BanditStore {
    pub fn get_or_create(&mut self, key: &str) -> &mut ThresholdBandit {
        self.bandits.entry(key.to_string()).or_default()
    }

    pub fn load(project_root: &str) -> Self {
        let path = bandit_path(project_root);
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(mut store) = serde_json::from_str::<BanditStore>(&content)
        {
            store.migrate_legacy_keys();
            return store;
        }
        Self::default()
    }

    pub fn save(&self, project_root: &str) -> Result<(), String> {
        let path = bandit_path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Converts legacy key formats to the unified `domain:ext:bucket` scheme.
    /// Reads old keys, writes new keys, removes old keys. Idempotent.
    fn migrate_legacy_keys(&mut self) {
        let old_keys: Vec<String> = self
            .bandits
            .keys()
            .filter(|k| !k.contains(':'))
            .cloned()
            .collect();

        for key in old_keys {
            let Some(bandit) = self.bandits.remove(&key) else {
                continue;
            };

            if let Some(ext) = key.strip_suffix("_feedback") {
                let new_key = bandit_key("feedback", ext, None);
                self.bandits.entry(new_key).or_insert(bandit);
            } else if let Some((ext, bucket)) = split_legacy_sized_key(&key) {
                let thresh_key = bandit_key("threshold", ext, Some(bucket));
                let mode_key = bandit_key("mode", ext, Some(bucket));
                self.bandits
                    .entry(thresh_key)
                    .or_insert_with(|| bandit.clone());
                self.bandits.entry(mode_key).or_insert(bandit);
            }
        }
    }

    pub fn format_report(&self) -> String {
        if self.bandits.is_empty() {
            return "No bandit data yet.".to_string();
        }
        let mut lines = vec!["Threshold Bandits (Thompson Sampling):".to_string()];
        for (key, bandit) in &self.bandits {
            lines.push(format!("  {key} (pulls: {}):", bandit.total_pulls));
            for arm in &bandit.arms {
                let mean = arm.mean();
                lines.push(format!(
                    "    {}: α={:.1} β={:.1} mean={:.0}% entropy={:.2} jaccard={:.2} budget={:.0}%",
                    arm.name,
                    arm.alpha,
                    arm.beta,
                    mean * 100.0,
                    arm.entropy_threshold,
                    arm.jaccard_threshold,
                    arm.budget_ratio * 100.0
                ));
            }
        }
        lines.join("\n")
    }
}

fn bandit_path(project_root: &str) -> std::path::PathBuf {
    let hash = crate::core::project_hash::hash_project_root(project_root);
    crate::core::data_dir::lean_ctx_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("projects")
        .join(hash)
        .join("bandits.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bandit_default_has_three_arms() {
        let b = ThresholdBandit::default();
        assert_eq!(b.arms.len(), 3);
        assert_eq!(b.arms[0].name, "conservative");
        assert_eq!(b.arms[1].name, "balanced");
        assert_eq!(b.arms[2].name, "aggressive");
    }

    #[test]
    fn bandit_selection_works() {
        let mut b = ThresholdBandit::default();
        for _ in 0..10 {
            let arm = b.choose_arm();
            let _ = arm.name.clone();
        }
        assert_eq!(b.total_pulls, 10);
    }

    #[test]
    fn choose_arm_is_deterministic_by_default() {
        // #4 / #498: with stochastic exploration off (test default), choose_arm
        // returns the argmax-of-mean arm every time — reproducible.
        let mut a = ThresholdBandit::default();
        let mut b = ThresholdBandit::default();
        // Train both identically toward "aggressive".
        for _ in 0..15 {
            a.update("aggressive", true);
            b.update("aggressive", true);
        }
        let pick_a: Vec<String> = (0..5).map(|_| a.choose_arm().name.clone()).collect();
        let pick_b: Vec<String> = (0..5).map(|_| b.choose_arm().name.clone()).collect();
        assert_eq!(pick_a, pick_b, "selection must be reproducible");
        assert!(
            pick_a.iter().all(|n| n == "aggressive"),
            "argmax should pick the trained arm, got {pick_a:?}"
        );
    }

    #[test]
    fn best_arm_by_mean_picks_highest_posterior() {
        let mut b = ThresholdBandit::default();
        for _ in 0..10 {
            b.update("balanced", true);
        }
        assert_eq!(b.arms[b.best_arm_idx_by_mean()].name, "balanced");
    }

    #[test]
    fn bandit_update_shifts_distribution() {
        let mut b = ThresholdBandit::default();
        for _ in 0..20 {
            b.update("aggressive", true);
        }
        for _ in 0..20 {
            b.update("conservative", false);
        }
        let agg = b.arms.iter().find(|a| a.name == "aggressive").unwrap();
        let con = b.arms.iter().find(|a| a.name == "conservative").unwrap();
        assert!(agg.mean() > con.mean());
    }

    #[test]
    fn store_save_load_roundtrip() {
        let _env = crate::core::data_dir::test_env_lock();
        let data_dir = tempfile::tempdir().unwrap();
        crate::test_env::set_var("LEAN_CTX_DATA_DIR", data_dir.path());

        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_string_lossy().to_string();
        let mut store = BanditStore::default();
        store.get_or_create(&bandit_key("mode", "rs", Some("md")));
        store.save(&root).unwrap();
        let loaded = BanditStore::load(&root);
        assert!(
            loaded
                .bandits
                .contains_key(&bandit_key("mode", "rs", Some("md")))
        );
    }

    #[test]
    fn migrate_legacy_feedback_key() {
        let mut store = BanditStore::default();
        let mut b = ThresholdBandit {
            total_pulls: 42,
            ..ThresholdBandit::default()
        };
        b.update("aggressive", true);
        store.bandits.insert("rs_feedback".to_string(), b);

        store.migrate_legacy_keys();

        assert!(
            !store.bandits.contains_key("rs_feedback"),
            "old key must be removed"
        );
        assert!(
            store
                .bandits
                .contains_key(&bandit_key("feedback", "rs", None)),
            "new key must exist"
        );
        assert_eq!(
            store.bandits[&bandit_key("feedback", "rs", None)].total_pulls,
            42
        );
    }

    #[test]
    fn migrate_legacy_sized_key_splits_into_threshold_and_mode() {
        let mut store = BanditStore::default();
        let b = ThresholdBandit {
            total_pulls: 10,
            ..ThresholdBandit::default()
        };
        store.bandits.insert("rs_md".to_string(), b);

        store.migrate_legacy_keys();

        assert!(
            !store.bandits.contains_key("rs_md"),
            "old key must be removed"
        );
        assert!(
            store
                .bandits
                .contains_key(&bandit_key("threshold", "rs", Some("md"))),
            "threshold key must exist"
        );
        assert!(
            store
                .bandits
                .contains_key(&bandit_key("mode", "rs", Some("md"))),
            "mode key must exist"
        );
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut store = BanditStore::default();
        store.get_or_create(&bandit_key("feedback", "rs", None));
        store.migrate_legacy_keys();
        store.migrate_legacy_keys();
        assert_eq!(store.bandits.len(), 1);
    }
}
