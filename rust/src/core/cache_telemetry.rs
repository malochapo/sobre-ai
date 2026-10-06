//! Read-cache reliability telemetry.
//!
//! The subjective "re-reads feel unreliable" signal is turned into data here:
//! every event that wipes or invalidates a *fully-delivered* entry — and thus
//! forces the next read to re-send the whole file instead of the cheap
//! `[unchanged]` stub — increments a process-global counter grouped by cause.
//!
//! Counters are monotonic `AtomicU64`s. They are surfaced ONLY in the
//! `ctx_cache status` diagnostic, never inside a cacheable tool-output body, so
//! output determinism (#498) is preserved.

use std::sync::atomic::{AtomicU64, Ordering};

static COMPACTION: AtomicU64 = AtomicU64::new(0);
static IDLE: AtomicU64 = AtomicU64::new(0);
static EVICTION: AtomicU64 = AtomicU64::new(0);
static CONVERSATION: AtomicU64 = AtomicU64::new(0);
static RAW_CAP_COUNT: AtomicU64 = AtomicU64::new(0);
static RAW_CAP_PREVENTED: AtomicU64 = AtomicU64::new(0);

/// Fully-delivered entries whose delivery flag was reset by a host compaction.
pub fn record_compaction(n: u64) {
    if n > 0 {
        COMPACTION.fetch_add(n, Ordering::Relaxed);
    }
}

/// Fully-delivered entries dropped by an idle-TTL cache clear.
pub fn record_idle(n: u64) {
    if n > 0 {
        IDLE.fetch_add(n, Ordering::Relaxed);
    }
}

/// Fully-delivered entries evicted under RAM / token-budget pressure.
pub fn record_eviction(n: u64) {
    if n > 0 {
        EVICTION.fetch_add(n, Ordering::Relaxed);
    }
}

/// A re-read that fell back to full content because the reading conversation
/// differed from the one the entry was delivered to (conversation scoping, #954).
pub fn record_conversation_mismatch() {
    CONVERSATION.fetch_add(1, Ordering::Relaxed);
}

/// Framed output was larger than raw content — `cap_to_raw()` fell back to
/// verbatim to prevent negative savings. Tracked so the dashboard can
/// distinguish "no savings" from "savings prevented inflation".
pub fn record_raw_cap(prevented_inflation_tokens: u64) {
    RAW_CAP_COUNT.fetch_add(1, Ordering::Relaxed);
    RAW_CAP_PREVENTED.fetch_add(prevented_inflation_tokens, Ordering::Relaxed);
}
