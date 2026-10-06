//! Cache-aligner telemetry (#940): how much of the provider prompt cache the
//! client's unanchored system prompt leaks through volatile fields. A
//! measurement-only signal — the body is never mutated — surfaced on `/status`.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Requests whose unanchored system prompt carried at least one volatile,
/// cache-busting field.
static VOLATILE_SYSTEM_REQUESTS: AtomicU64 = AtomicU64::new(0);
/// Cumulative volatile fields detected across those requests.
static VOLATILE_FIELDS_DETECTED: AtomicU64 = AtomicU64::new(0);

/// Record one unanchored-system scan that found `fields` volatile fields.
/// A no-op when none were found, so the gauges count only cache-leaking requests.
pub(crate) fn record_volatile_system(fields: u64) {
    if fields == 0 {
        return;
    }
    VOLATILE_SYSTEM_REQUESTS.fetch_add(1, Ordering::Relaxed);
    VOLATILE_FIELDS_DETECTED.fetch_add(fields, Ordering::Relaxed);
}

/// Point-in-time view of the cache-aligner counters for `/status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct CacheSafety {
    /// Requests whose unanchored system prompt leaked at least one volatile field.
    pub volatile_system_requests: u64,
    /// Volatile fields detected across those requests.
    pub volatile_fields_detected: u64,
}

#[must_use]
pub(crate) fn snapshot() -> CacheSafety {
    CacheSafety {
        volatile_system_requests: VOLATILE_SYSTEM_REQUESTS.load(Ordering::Relaxed),
        volatile_fields_detected: VOLATILE_FIELDS_DETECTED.load(Ordering::Relaxed),
    }
}
