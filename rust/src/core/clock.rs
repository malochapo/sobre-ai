//! Wall-clock helpers.

/// Seconds since the Unix epoch; 0 if the clock is set before it.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
