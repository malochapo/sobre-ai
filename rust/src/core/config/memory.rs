//! RAM cleanup profile and memory-footprint presets (`config.toml`).

use serde::{Deserialize, Serialize};

/// Controls how aggressively lean-ctx frees memory when idle.
/// - `aggressive`: Cache cleared after short idle period (5 min). Best for low-memory devices.
/// - `shared`: (Default) Cache retained for 1 hour. Best for typical agent sessions with think pauses.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCleanup {
    Aggressive,
    #[default]
    Shared,
}

impl MemoryCleanup {
    /// Idle TTL in seconds before cache is auto-cleared.
    pub fn idle_ttl_secs(&self) -> u64 {
        match self {
            Self::Aggressive => 300,
            Self::Shared => 3600,
        }
    }
}

/// Controls RAM usage vs. feature richness trade-off.
/// - `low`: Minimal RAM footprint, disables optional caches and embedding features
/// - `balanced`: Default — moderate caches, single embedding engine
/// - `performance`: Maximum caches, all features enabled
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryProfile {
    Low,
    Balanced,
    #[default]
    Performance,
}

impl MemoryProfile {
    pub fn bm25_max_cache_mb(&self) -> u64 {
        match self {
            Self::Low => 64,
            Self::Balanced => 128,
            Self::Performance => 512,
        }
    }

    pub fn embeddings_enabled(&self) -> bool {
        !matches!(self, Self::Low)
    }
}

/// Controls visibility of token savings footers in tool output.
///
/// - `always`: shown on every compressed response
/// - `never` (default): suppressed everywhere
/// - `auto`: legacy compatibility mode; behavior is transport/context dependent
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SavingsFooter {
    Auto,
    Always,
    #[default]
    Never,
}

/// RSS-based memory guardian configuration.
pub struct MemoryGuardConfig {
    pub max_ram_percent: u8,
}

impl Default for MemoryGuardConfig {
    fn default() -> Self {
        Self {
            max_ram_percent: super::MAX_RAM_PERCENT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn savings_footer_defaults_to_never() {
        assert_eq!(SavingsFooter::default(), SavingsFooter::Never);
    }
}
