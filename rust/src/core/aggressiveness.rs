//! Single 0.0–1.0 compression-intensity knob.
//!
//! `sobre_read`'s `aggressiveness` argument expresses "how hard should I
//! compress?" as one number instead of a read mode. The number is *mapped*
//! onto the existing density / entropy / information-bottleneck stages — it
//! never introduces a model, so the output stays a pure function of the
//! inputs. Without the argument ([`effective`] returns `None`) each mode keeps
//! its own default; `compression_level` in the config does not set it.

/// Concrete tuning derived from one aggressiveness value. Every field is a pure
/// function of `a`, so the same `a` always yields the same knobs (determinism).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AggressivenessProfile {
    /// Fraction of tokens to keep (density target for `density:` / IB prose).
    /// `a=0.0 → 1.00` (keep everything), `a=1.0 → 0.15` (keep ~15%).
    pub density_target: f64,
    /// BPE-entropy keep threshold for the `entropy` mode. Lines below this are
    /// dropped. `a=0.0 → 0.6` (keep almost all), `a=1.0 → 2.0` (drop low-info).
    pub bpe_entropy: f64,
    /// Information-bottleneck keep ratio for `task`/IB modes.
    /// `a=0.0 → 0.60`, `a=1.0 → 0.10`.
    pub ib_budget_ratio: f64,
}

impl AggressivenessProfile {
    /// Maps `a ∈ [0,1]` (clamped) onto the three tuning knobs. The constants
    /// are hand-set and monotonic; no measurement calibrated them.
    #[must_use]
    pub fn from_level(a: f64) -> Self {
        let a = a.clamp(0.0, 1.0);
        Self {
            density_target: (1.0 - 0.85 * a).clamp(0.10, 1.0),
            bpe_entropy: 0.6 + 1.4 * a,
            ib_budget_ratio: (0.6 - 0.5 * a).clamp(0.10, 0.6),
        }
    }
}

/// The effective read aggressiveness: the per-call value, clamped, or `None`
/// so the read keeps its automatic per-file mode. `compression_level` in the
/// config does not force read modes; it picks the prose style.
#[must_use]
pub fn effective(explicit: Option<f64>) -> Option<f64> {
    explicit.map(|a| a.clamp(0.0, 1.0))
}

/// Stable cache-key fragment for an aggressiveness setting.
///
/// Buckets to 1/20 (0.05 steps) so float jitter does not fragment the cache,
/// while distinct settings still get distinct keys. `None` → empty
/// string, so a read without the knob keeps the plain mode key.
#[must_use]
pub fn cache_fragment(a: Option<f64>) -> String {
    match a {
        Some(a) => format!("a{}", (a.clamp(0.0, 1.0) * 20.0).round() as u32),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_monotonic_in_aggressiveness() {
        let lo = AggressivenessProfile::from_level(0.0);
        let mid = AggressivenessProfile::from_level(0.5);
        let hi = AggressivenessProfile::from_level(1.0);

        // Higher aggressiveness keeps fewer tokens and drops more low-info lines.
        assert!(lo.density_target > mid.density_target);
        assert!(mid.density_target > hi.density_target);
        assert!(lo.bpe_entropy < mid.bpe_entropy);
        assert!(mid.bpe_entropy < hi.bpe_entropy);
        assert!(lo.ib_budget_ratio > mid.ib_budget_ratio);
        assert!(mid.ib_budget_ratio > hi.ib_budget_ratio);
    }

    #[test]
    fn profile_clamps_out_of_range() {
        assert_eq!(
            AggressivenessProfile::from_level(-1.0),
            AggressivenessProfile::from_level(0.0)
        );
        assert_eq!(
            AggressivenessProfile::from_level(2.0),
            AggressivenessProfile::from_level(1.0)
        );
    }

    #[test]
    fn cache_fragment_is_stable_and_bucketed() {
        // None → empty.
        assert_eq!(cache_fragment(None), "");
        // Same value → same fragment (determinism).
        assert_eq!(cache_fragment(Some(0.7)), cache_fragment(Some(0.7)));
        // Jitter within a bucket collapses; distinct buckets differ.
        assert_eq!(cache_fragment(Some(0.70)), cache_fragment(Some(0.701)));
        assert_ne!(cache_fragment(Some(0.70)), cache_fragment(Some(0.80)));
    }
}
