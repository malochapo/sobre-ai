//! Co-access matrix: files read together strengthen their association, which
//! decays with a half-life (Hebb: "fire together, wire together").

use std::collections::HashMap;
use std::time::Instant;

/// Maximum number of co-access pairs tracked (prevents unbounded growth).
const MAX_ASSOCIATIONS: usize = 10_000;
/// Decay half-life in seconds for Hebbian weights.
const DECAY_HALF_LIFE_SECS: f64 = 300.0;
/// Minimum weight before pruning.
const PRUNE_THRESHOLD: f32 = 0.01;

/// Tracks co-access patterns between files (Hebbian learning).
pub struct CoAccessMatrix {
    /// Sparse co-access weights: (path_hash_a, path_hash_b) → weight
    weights: HashMap<(u64, u64), f32>,
    /// When each pair was last strengthened
    timestamps: HashMap<(u64, u64), Instant>,
    /// Current access burst (files read in the same tool-call window)
    current_burst: Vec<u64>,
    burst_start: Instant,
}

impl Default for CoAccessMatrix {
    fn default() -> Self {
        Self::new()
    }
}

impl CoAccessMatrix {
    pub fn new() -> Self {
        Self {
            weights: HashMap::with_capacity(256),
            timestamps: HashMap::with_capacity(256),
            current_burst: Vec::with_capacity(8),
            burst_start: Instant::now(),
        }
    }

    /// Record a file access. If within the burst window (500ms), strengthens
    /// associations with other files in the same burst.
    pub fn record_access(&mut self, path_hash: u64) {
        let now = Instant::now();
        let burst_window = std::time::Duration::from_millis(500);

        if now.duration_since(self.burst_start) > burst_window {
            self.flush_burst();
            self.burst_start = now;
        }

        self.current_burst.push(path_hash);
    }

    /// Flush current burst: strengthen all pairwise associations.
    fn flush_burst(&mut self) {
        if self.current_burst.len() < 2 {
            self.current_burst.clear();
            return;
        }

        let now = Instant::now();
        let burst = std::mem::take(&mut self.current_burst);

        for i in 0..burst.len() {
            for j in (i + 1)..burst.len() {
                let key = normalized_key(burst[i], burst[j]);
                let w = self.weights.entry(key).or_insert(0.0);
                *w += 1.0;
                self.timestamps.insert(key, now);
            }
        }

        if self.weights.len() > MAX_ASSOCIATIONS {
            self.prune();
        }
    }

    /// Get the association strength of a file with all currently active files.
    /// Applies exponential decay based on elapsed time.
    pub fn association_strength(&self, path_hash: u64, active_hashes: &[u64]) -> f32 {
        let now = Instant::now();
        let mut total = 0.0f32;

        for &active in active_hashes {
            let key = normalized_key(path_hash, active);
            if let Some(&weight) = self.weights.get(&key) {
                let elapsed = self.timestamps.get(&key).map_or(DECAY_HALF_LIFE_SECS, |t| {
                    now.duration_since(*t).as_secs_f64()
                });
                let decay = (-elapsed * (2.0f64.ln()) / DECAY_HALF_LIFE_SECS).exp();
                total += weight * decay as f32;
            }
        }

        total
    }

    /// Remove weak associations to keep memory bounded.
    fn prune(&mut self) {
        let now = Instant::now();
        self.weights.retain(|key, weight| {
            let elapsed = self
                .timestamps
                .get(key)
                .map_or(DECAY_HALF_LIFE_SECS * 2.0, |t| {
                    now.duration_since(*t).as_secs_f64()
                });
            let decay = (-elapsed * (2.0f64.ln()) / DECAY_HALF_LIFE_SECS).exp();
            let effective = *weight * decay as f32;
            if effective < PRUNE_THRESHOLD {
                self.timestamps.remove(key);
                false
            } else {
                true
            }
        });

        // Hard cap: a high-churn burst of strong, fresh associations can leave the map
        // above MAX_ASSOCIATIONS after threshold-pruning. Drop the lowest-effective-weight
        // pairs to enforce the cap (keeping both maps key-synced). Tradeoff: discards the
        // weakest learned co-access pairs, which are re-learned if seen again.
        if self.weights.len() > MAX_ASSOCIATIONS {
            let mut scored: Vec<((u64, u64), f32)> = self
                .weights
                .iter()
                .map(|(&key, &weight)| {
                    let elapsed = self
                        .timestamps
                        .get(&key)
                        .map_or(DECAY_HALF_LIFE_SECS * 2.0, |t| {
                            now.duration_since(*t).as_secs_f64()
                        });
                    let decay = (-elapsed * (2.0f64.ln()) / DECAY_HALF_LIFE_SECS).exp();
                    (key, weight * decay as f32)
                })
                .collect();
            scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let to_drop = self.weights.len() - MAX_ASSOCIATIONS;
            for (key, _) in scored.into_iter().take(to_drop) {
                self.weights.remove(&key);
                self.timestamps.remove(&key);
            }
        }
    }

    /// Force flush any pending burst (call at end of tool-call processing).
    pub fn end_burst(&mut self) {
        self.flush_burst();
    }
}

/// Normalize key so (a,b) == (b,a).
fn normalized_key(a: u64, b: u64) -> (u64, u64) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Compute a fast hash for a file path.
pub fn path_hash(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn co_access_strengthens_pairs() {
        let mut matrix = CoAccessMatrix::new();
        let a = path_hash("src/main.rs");
        let b = path_hash("src/lib.rs");
        let c = path_hash("src/config.rs");

        // Simulate burst: A, B, C accessed together
        matrix.record_access(a);
        matrix.record_access(b);
        matrix.record_access(c);
        matrix.end_burst();

        // A should have association with B
        assert!(matrix.association_strength(a, &[b]) > 0.0);
        // And with C
        assert!(matrix.association_strength(a, &[c]) > 0.0);
    }

    #[test]
    fn unrelated_files_have_zero_association() {
        let matrix = CoAccessMatrix::new();
        let a = path_hash("src/main.rs");
        let b = path_hash("src/lib.rs");
        assert_eq!(matrix.association_strength(a, &[b]), 0.0);
    }

    #[test]
    fn normalized_key_is_symmetric() {
        assert_eq!(normalized_key(42, 99), normalized_key(99, 42));
    }
}
