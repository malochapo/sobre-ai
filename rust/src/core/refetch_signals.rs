//! Re-fetch signals: the agent asking again for what sobre already gave it.
//!
//! A fresh re-read of a file, a `map` read bounced to `full`, the same shell
//! command re-run within a minute, or a `sobre_expand` of compressed output all
//! mean the compressed form lost something the agent needed. The counts feed
//! [`crate::core::config::CompressionLevel::degrade_action`], which can only
//! lower compression for the session. Nothing here throttles or refuses a call.

use std::collections::HashMap;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_mins(2);
const MODE_BOUNCE_WINDOW: Duration = Duration::from_secs(30);
const SHELL_RERUN_WINDOW: Duration = Duration::from_mins(1);
const COLD_START_CALLS: u32 = 3;

/// Sliding-window record of re-fetch signals for one server session.
#[derive(Debug, Default)]
pub struct RefetchSignals {
    corrections: Vec<Instant>,
    recent_reads: HashMap<String, (Instant, String)>,
    recent_commands: HashMap<String, Instant>,
    total_calls: u32,
    retrieves: Vec<Instant>,
}

impl RefetchSignals {
    /// Records a `sobre_read`: a `fresh=true` re-read of a file read in the
    /// window, or a `map`/`signatures` read followed by `full` within 30s.
    pub fn record_read(&mut self, path: &str, mode: &str, fresh: bool) {
        self.total_calls += 1;
        let now = Instant::now();

        if self.total_calls > COLD_START_CALLS
            && let Some((prev_time, prev_mode)) = self.recent_reads.get(path)
        {
            let age = now.duration_since(*prev_time);
            let fresh_reread = fresh && age < WINDOW;
            let bounce = mode == "full"
                && (prev_mode == "map" || prev_mode == "signatures")
                && age < MODE_BOUNCE_WINDOW;
            if fresh_reread {
                self.corrections.push(now);
            }
            if bounce {
                self.corrections.push(now);
            }
        }
        self.recent_reads
            .insert(path.to_string(), (now, mode.to_string()));
    }

    /// Records a `sobre_shell` command: the same command re-run within 60s.
    pub fn record_shell(&mut self, command: &str) {
        self.total_calls += 1;
        let now = Instant::now();
        let key = normalize_shell_command(command);
        if self.total_calls > COLD_START_CALLS
            && let Some(prev_time) = self.recent_commands.get(&key)
            && now.duration_since(*prev_time) < SHELL_RERUN_WINDOW
        {
            self.corrections.push(now);
        }
        self.recent_commands.insert(key, now);
    }

    /// Records one `sobre_expand` of compressed output (#941).
    pub fn record_retrieve(&mut self) {
        self.retrieves.push(Instant::now());
    }

    /// Re-reads and re-runs in the window.
    pub fn correction_count(&self) -> u32 {
        count_recent(&self.corrections)
    }

    /// `sobre_expand` calls in the window.
    pub fn retrieve_count(&self) -> u32 {
        count_recent(&self.retrieves)
    }

    /// Drops signals and entries older than the window.
    pub fn prune(&mut self) {
        let now = Instant::now();
        self.corrections.retain(|t| now.duration_since(*t) < WINDOW);
        self.retrieves.retain(|t| now.duration_since(*t) < WINDOW);
        self.recent_reads
            .retain(|_, (t, _)| now.duration_since(*t) < WINDOW);
        self.recent_commands
            .retain(|_, t| now.duration_since(*t) < WINDOW);
    }
}

fn count_recent(signals: &[Instant]) -> u32 {
    let now = Instant::now();
    signals
        .iter()
        .filter(|t| now.duration_since(**t) < WINDOW)
        .count() as u32
}

fn normalize_shell_command(cmd: &str) -> String {
    cmd.split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warm(signals: &mut RefetchSignals) {
        for i in 0..COLD_START_CALLS {
            signals.record_read(&format!("warmup{i}.rs"), "full", false);
        }
    }

    #[test]
    fn fresh_reread_is_a_signal() {
        let mut s = RefetchSignals::default();
        warm(&mut s);
        s.record_read("src/main.rs", "full", false);
        assert_eq!(s.correction_count(), 0);
        s.record_read("src/main.rs", "full", true);
        assert_eq!(s.correction_count(), 1);
    }

    #[test]
    fn map_then_full_is_a_signal() {
        let mut s = RefetchSignals::default();
        warm(&mut s);
        s.record_read("src/cache.rs", "map", false);
        assert_eq!(s.correction_count(), 0);
        s.record_read("src/cache.rs", "full", false);
        assert_eq!(s.correction_count(), 1);
    }

    #[test]
    fn shell_rerun_is_a_signal() {
        let mut s = RefetchSignals::default();
        warm(&mut s);
        s.record_shell("cargo test --lib");
        assert_eq!(s.correction_count(), 0);
        s.record_shell("cargo test --lib");
        assert_eq!(s.correction_count(), 1);
    }

    #[test]
    fn cold_start_is_ignored() {
        let mut s = RefetchSignals::default();
        s.record_shell("cargo check");
        s.record_shell("cargo check");
        s.record_shell("cargo check");
        assert_eq!(s.correction_count(), 0);
    }

    #[test]
    fn retrieves_are_counted_separately() {
        let mut s = RefetchSignals::default();
        s.record_retrieve();
        s.record_retrieve();
        s.record_retrieve();
        assert_eq!(s.retrieve_count(), 3);
        assert_eq!(s.correction_count(), 0);
        s.prune();
        assert_eq!(s.retrieve_count(), 3, "signals inside the window survive");
    }
}
