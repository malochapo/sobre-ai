//! Configuration enums and their behavior.
//!
//! Extracted from `config::mod` to keep the top-level config module focused on
//! the `Config` struct and loading logic. These types are re-exported from the
//! `config` module root, so external paths like `config::CompressionLevel`
//! continue to work unchanged.

use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicU8;

use super::Config;

static SESSION_DEGRADE_LEVEL: AtomicU8 = AtomicU8::new(0);

/// Each level maps to specific component settings via `to_components()`.
/// Variants are declared from least to most compressed, so `<` compares them.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum CompressionLevel {
    Off,
    /// Default: plain-English "concise" guidance (bullets, no filler). Readable
    /// by humans inspecting their rules files, and still token-saving. The
    /// denser, symbolic styles (`Standard`/`Max`, which enable CRP and the
    /// `→ ∵ ∴` vocabulary) are opt-in "power modes" — set `compression_level`
    /// in config. This only shapes the model's prose; tool-output compression
    /// is governed separately and is unaffected.
    #[default]
    Lite,
    Standard,
    Max,
    /// Ultra-dense bullet-point-only mode — zero prose, diff-style facts,
    /// strictest token budget (<=50 tokens per non-code response). Maps to
    /// CrpMode::Tdd with tighter output constraints (#795).
    Raw,
}

/// Outcome of [`CompressionLevel::degrade_action`]: what to do with the session
/// degrade given the current re-fetch pressure. Split from the dispatch so the
/// threshold logic is a pure, testable function.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionDegrade {
    /// Set the session degrade to this level.
    Set(CompressionLevel),
    /// Clear any session degrade (pressure fully relaxed).
    Clear,
    /// Leave the current degrade unchanged (intermediate pressure band).
    Leave,
}

impl CompressionLevel {
    /// The CRP mode and whether the session is terse at this level.
    pub fn to_components(&self) -> (&'static str, bool) {
        match self {
            Self::Off => ("off", false),
            Self::Lite => ("off", true),
            Self::Standard => ("compact", true),
            Self::Max | Self::Raw => ("tdd", true),
        }
    }

    /// Returns the effective prose level: the band of `compression_level`,
    /// lowered by a session degrade (re-fetch feedback) but never raised.
    pub fn effective(config: &Config) -> Self {
        Self::capped(
            Self::from_value(config.compression_level),
            Self::session_degrade_level(),
        )
    }

    /// The level `compression_level` alone selects, ignoring any degrade.
    pub fn effective_configured(config: &Config) -> Self {
        Self::from_value(config.compression_level)
    }

    fn capped(configured: Self, degraded: Option<Self>) -> Self {
        degraded.map_or(configured, |d| d.min(configured))
    }

    /// The prose level for a continuous `compression_level` value: five equal
    /// bands over `[0, 1]` (off, lite, standard, max, raw).
    #[must_use]
    pub fn from_value(value: f64) -> Self {
        match value.clamp(0.0, 1.0) {
            v if v < 0.2 => Self::Off,
            v if v < 0.4 => Self::Lite,
            v if v < 0.6 => Self::Standard,
            v if v < 0.8 => Self::Max,
            _ => Self::Raw,
        }
    }

    /// The `compression_level` value a level name stands for: `off` is 0, the
    /// others sit in the middle of their band.
    #[must_use]
    pub fn value(self) -> f64 {
        match self {
            Self::Off => 0.0,
            Self::Lite => 0.3,
            Self::Standard => 0.5,
            Self::Max => 0.7,
            Self::Raw => 0.9,
        }
    }

    /// Session-level degrade: re-fetches detected, temporarily reduce compression.
    /// 0 = no override, 1 = Off, 2 = Lite
    pub fn session_degrade_level() -> Option<Self> {
        match SESSION_DEGRADE_LEVEL.load(std::sync::atomic::Ordering::Relaxed) {
            1 => Some(Self::Off),
            2 => Some(Self::Lite),
            _ => None,
        }
    }

    /// Sets a session-level compression degrade (called on re-fetch pressure).
    pub fn set_session_degrade(level: &Self) {
        let val = match level {
            Self::Off => 1u8,
            Self::Lite => 2u8,
            _ => 0u8,
        };
        SESSION_DEGRADE_LEVEL.store(val, std::sync::atomic::Ordering::Relaxed);
    }

    /// Clears the session-level degrade (recovery after correction rate drops).
    pub fn clear_session_degrade() {
        SESSION_DEGRADE_LEVEL.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Maps re-fetch *pressure* to a session-degrade decision. Pressure is the
    /// stronger of the correction-loop count (re-reads/re-runs) and the CCR
    /// retrieve count (`ctx_expand`) — two views of the same "too
    /// aggressive" signal (#941): 5+ degrades to `Off`, 3+ to `Lite`, 0 clears,
    /// and the 1–2 band leaves the current degrade untouched.
    ///
    /// Pure and total so the thresholds are unit-testable without the dispatch
    /// path — the regression guard for the brittle source-grep test this replaced
    /// (#957).
    pub fn degrade_action(correction_count: u32, retrieve_count: u32) -> SessionDegrade {
        let pressure = correction_count.max(retrieve_count);
        if pressure >= 5 {
            SessionDegrade::Set(Self::Off)
        } else if pressure >= 3 {
            SessionDegrade::Set(Self::Lite)
        } else if pressure == 0 {
            SessionDegrade::Clear
        } else {
            SessionDegrade::Leave
        }
    }

    /// Applies a [`SessionDegrade`] decision to the process-global session
    /// state. Returns the effective level when it changed, so the caller can
    /// say so in the tool output.
    pub fn apply_degrade_action(action: SessionDegrade, config: &Config) -> Option<Self> {
        let before = Self::effective(config);
        match action {
            SessionDegrade::Set(level) => Self::set_session_degrade(&level),
            SessionDegrade::Clear => Self::clear_session_degrade(),
            SessionDegrade::Leave => {}
        }
        let after = Self::effective(config);
        (after != before).then_some(after)
    }

    pub fn from_str_label(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "lite" => Some(Self::Lite),
            "standard" | "std" => Some(Self::Standard),
            "max" => Some(Self::Max),
            "raw" => Some(Self::Raw),
            _ => None,
        }
    }

    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lite => "lite",
            Self::Standard => "standard",
            Self::Max => "max",
            Self::Raw => "raw",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Off => "No compression — full verbose output",
            Self::Lite => "Light compression — concise output, basic terse filtering",
            Self::Standard => {
                "Standard compression — dense output, compact protocol, pattern-aware"
            }
            Self::Max => "Maximum compression — expert mode, TDD protocol, all layers active",
            Self::Raw => "Raw-dense compression — bullet points only, zero prose, diff-style facts",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CompressionLevel, SessionDegrade};

    #[test]
    fn degrade_action_thresholds_are_pressure_based() {
        use CompressionLevel::{Lite, Off};
        use SessionDegrade::{Clear, Leave, Set};
        // 5+ pressure → Off, driven by EITHER the correction-loop or the CCR
        // retrieve count (the stronger of the two), per #941.
        assert_eq!(CompressionLevel::degrade_action(5, 0), Set(Off));
        assert_eq!(CompressionLevel::degrade_action(0, 5), Set(Off));
        assert_eq!(CompressionLevel::degrade_action(9, 1), Set(Off));
        // 3–4 pressure → Lite.
        assert_eq!(CompressionLevel::degrade_action(3, 0), Set(Lite));
        assert_eq!(CompressionLevel::degrade_action(0, 4), Set(Lite));
        assert_eq!(CompressionLevel::degrade_action(4, 4), Set(Lite));
        // 0 pressure → clear; the 1–2 band holds the current degrade.
        assert_eq!(CompressionLevel::degrade_action(0, 0), Clear);
        assert_eq!(CompressionLevel::degrade_action(2, 1), Leave);
        assert_eq!(CompressionLevel::degrade_action(1, 2), Leave);
    }

    #[test]
    fn degrade_lowers_but_never_raises_the_configured_level() {
        use CompressionLevel::{Lite, Max, Off};
        assert_eq!(CompressionLevel::capped(Max, Some(Lite)), Lite);
        assert_eq!(CompressionLevel::capped(Off, Some(Lite)), Off);
        assert_eq!(CompressionLevel::capped(Max, None), Max);
    }
}
