//! Quality loop v1: compression-caused edit failures feed back into
//! mode selection.
//!
//! `BounceTracker`/`path_mode_memory` close the loop for *re-read* bounces,
//! but an edit that fails because the file was last read in a compressed mode
//! (`old_text` not found — the body simply wasn't in context) taught the
//! system nothing. This module records edit outcomes correlated with the last
//! read mode and feeds two signals back into `auto_mode_resolver::resolve`:
//!
//! 1. **Per-path escalation** — after a compression-correlated edit failure
//!    the *next* auto read of that file resolves to `full` (one-shot, 1 h TTL).
//! 2. **Per-(extension × mode) penalty** — modes whose edit-failure rate for a
//!    file type crosses the risky threshold resolve to `full` until the rate
//!    recovers (hysteresis, see below).
//!
//! Risk formula:
//! a (ext, mode) pair becomes risky when `fails >= 2 && fails / (fails +
//! successes) >= 0.25`, and stops being risky only when the rate drops below
//! `0.15` — two thresholds so one lucky edit doesn't flap the decision.
//!
//! Storage: memory only, for the life of the process. Nothing is written to
//! disk, so one run's edits never change how a later run reads.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Pending per-path escalations expire after this long.
const ESCALATION_TTL_SECS: u64 = 3600;
/// Hard caps; oldest entries are evicted first.
const MAX_PAIRS: usize = 200;
const MAX_PENDING: usize = 100;

/// Risky when the failure share reaches this rate (with >= 2 fails)…
const RISKY_ENTER_RATE: f64 = 0.25;
/// …and recovers only once the rate drops below this (hysteresis).
const RISKY_EXIT_RATE: f64 = 0.15;
const RISKY_MIN_FAILS: u32 = 2;

static STORE: OnceLock<Mutex<EditQualityStore>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
pub struct PairStats {
    pub fails: u32,
    pub successes: u32,
    pub risky: bool,
    pub last_fail_unix: u64,
}

impl PairStats {
    fn fail_rate(&self) -> f64 {
        let total = self.fails + self.successes;
        if total == 0 {
            return 0.0;
        }
        f64::from(self.fails) / f64::from(total)
    }

    /// Applies the documented enter/exit thresholds after every outcome.
    fn update_risky(&mut self) {
        if self.risky {
            if self.fail_rate() < RISKY_EXIT_RATE {
                self.risky = false;
            }
        } else if self.fails >= RISKY_MIN_FAILS && self.fail_rate() >= RISKY_ENTER_RATE {
            self.risky = true;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EditQualityStore {
    /// Key: `"{ext}|{mode}"` (e.g. `"rs|map"`).
    pub pairs: HashMap<String, PairStats>,
    /// Normalized path -> unix time of the compression-correlated edit fail.
    pub pending_escalations: HashMap<String, u64>,
    /// Normalized path -> unix time of an anchored-edit (`sobre_patch`) staleness
    /// miss. The next auto read of that path resolves to `anchored` (not `full`),
    /// so the model gets fresh line anchors to retry by reference.
    pub pending_anchored_escalations: HashMap<String, u64>,
}

fn pair_key(ext: &str, mode: &str) -> String {
    format!("{ext}|{mode}")
}

/// Evict the oldest entries of a `path -> timestamp` pending map down to
/// [`MAX_PENDING`]. Shared by the `full` and `anchored` escalation maps.
fn evict_pending_to_cap(map: &mut HashMap<String, u64>) {
    if map.len() <= MAX_PENDING {
        return;
    }
    let mut items: Vec<(String, u64)> = map.iter().map(|(k, ts)| (k.clone(), *ts)).collect();
    items.sort_by_key(|(_, ts)| *ts);
    let drop_n = map.len() - MAX_PENDING;
    for (key, _) in items.into_iter().take(drop_n) {
        map.remove(&key);
    }
}

impl EditQualityStore {
    fn evict_to_caps(&mut self) {
        if self.pairs.len() > MAX_PAIRS {
            let mut items: Vec<(String, u64)> = self
                .pairs
                .iter()
                .map(|(k, s)| (k.clone(), s.last_fail_unix))
                .collect();
            items.sort_by_key(|(_, ts)| *ts);
            let drop_n = self.pairs.len() - MAX_PAIRS;
            for (key, _) in items.into_iter().take(drop_n) {
                self.pairs.remove(&key);
            }
        }
        evict_pending_to_cap(&mut self.pending_escalations);
        evict_pending_to_cap(&mut self.pending_anchored_escalations);
    }

    pub fn record_failure(&mut self, ext: &str, mode: &str, now: u64) {
        let entry = self.pairs.entry(pair_key(ext, mode)).or_default();
        entry.fails = entry.fails.saturating_add(1);
        entry.last_fail_unix = now;
        entry.update_risky();
        self.evict_to_caps();
    }

    pub fn record_success(&mut self, ext: &str, mode: &str) {
        let entry = self.pairs.entry(pair_key(ext, mode)).or_default();
        entry.successes = entry.successes.saturating_add(1);
        entry.update_risky();
    }

    pub fn set_pending_escalation(&mut self, norm_path: &str, now: u64) {
        self.pending_escalations.insert(norm_path.to_string(), now);
        self.evict_to_caps();
    }

    /// Consumes the escalation for this path if present and not expired.
    pub fn take_pending_escalation(&mut self, norm_path: &str, now: u64) -> bool {
        Self::take_from(&mut self.pending_escalations, norm_path, now)
    }

    pub fn set_pending_anchored_escalation(&mut self, norm_path: &str, now: u64) {
        self.pending_anchored_escalations
            .insert(norm_path.to_string(), now);
        self.evict_to_caps();
    }

    /// Consumes the anchored escalation for this path if present and not expired.
    pub fn take_pending_anchored_escalation(&mut self, norm_path: &str, now: u64) -> bool {
        Self::take_from(&mut self.pending_anchored_escalations, norm_path, now)
    }

    /// Shared one-shot consume: remove `norm_path`; true when it was still
    /// within [`ESCALATION_TTL_SECS`].
    fn take_from(map: &mut HashMap<String, u64>, norm_path: &str, now: u64) -> bool {
        map.remove(norm_path)
            .is_some_and(|ts| now.saturating_sub(ts) <= ESCALATION_TTL_SECS)
    }

    pub fn is_risky(&self, ext: &str, mode: &str) -> bool {
        self.pairs
            .get(&pair_key(ext, mode))
            .is_some_and(|s| s.risky)
    }
}

fn global() -> &'static Mutex<EditQualityStore> {
    STORE.get_or_init(|| Mutex::new(EditQualityStore::default()))
}

fn ext_of(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}

/// Process-global: record the outcome of an edit, correlated with the mode of
/// the last read of that file. `last_mode` must be the recorded read mode
/// (empty = file was never read through sobre → no signal, skipped).
/// Compression-correlated failures additionally arm the one-shot per-path
/// escalation so the next auto read of `path` resolves to `full`.
pub fn record_edit_outcome(path: &str, last_mode: &str, success: bool) {
    record_outcome_with(path, last_mode, success, Escalation::Full);
}

/// Like [`record_edit_outcome`], but a failure is a `sobre_patch` anchor-staleness
/// miss: the recovery is a *fresh anchored read* (the model edits by reference),
/// so the next auto read escalates to `anchored` instead of `full`.
pub fn record_anchored_edit_outcome(path: &str, last_mode: &str, success: bool) {
    record_outcome_with(path, last_mode, success, Escalation::Anchored);
}

/// Which read mode the *next* auto read escalates to after a correlated edit
/// failure. Both are high-signal "the context the model edited against was
/// wrong" events; they differ only in the recovery view handed back.
#[derive(Clone, Copy)]
enum Escalation {
    /// str_replace miss → give the real body (`full`).
    Full,
    /// anchored miss → give fresh line anchors (`anchored`).
    Anchored,
}

impl Escalation {
    /// The read mode that fully neutralizes this failure class, hence the value
    /// to *not* re-arm against (escalating `full→full` / `anchored→anchored` is a
    /// no-op).
    fn target_mode(self) -> &'static str {
        match self {
            Escalation::Full => "full",
            Escalation::Anchored => "anchored",
        }
    }
}

fn record_outcome_with(path: &str, last_mode: &str, success: bool, esc: Escalation) {
    if last_mode.is_empty() {
        return;
    }
    let ext = ext_of(path);
    let Ok(mut store) = global().lock() else {
        return;
    };
    if success {
        store.record_success(&ext, last_mode);
    } else {
        let now = crate::core::clock::now_unix();
        store.record_failure(&ext, last_mode, now);
        if last_mode != esc.target_mode() {
            let norm = crate::core::pathutil::normalize_tool_path(path);
            match esc {
                Escalation::Full => store.set_pending_escalation(&norm, now),
                Escalation::Anchored => store.set_pending_anchored_escalation(&norm, now),
            }
        }
    }
}

/// Process-global: one-shot check-and-consume of the per-path `full` escalation.
pub fn take_pending_escalation(path: &str) -> bool {
    consume_escalation(path, false)
}

/// Process-global: one-shot check-and-consume of the per-path `anchored`
/// escalation (armed by [`record_anchored_edit_outcome`]).
pub fn take_pending_anchored_escalation(path: &str) -> bool {
    consume_escalation(path, true)
}

fn consume_escalation(path: &str, anchored: bool) -> bool {
    let norm = crate::core::pathutil::normalize_tool_path(path);
    let Ok(mut store) = global().lock() else {
        return false;
    };
    let now = crate::core::clock::now_unix();
    if anchored {
        store.take_pending_anchored_escalation(&norm, now)
    } else {
        store.take_pending_escalation(&norm, now)
    }
}

/// Process-global: is `mode` currently risky for files with this extension?
pub fn is_risky_mode(path: &str, mode: &str) -> bool {
    let ext = ext_of(path);
    global().lock().is_ok_and(|s| s.is_risky(&ext, mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_after_two_majority_fails_with_hysteresis() {
        let mut s = EditQualityStore::default();
        s.record_failure("rs", "map", 1000);
        assert!(!s.is_risky("rs", "map"), "one fail is not a pattern");
        s.record_failure("rs", "map", 1001);
        assert!(s.is_risky("rs", "map"), "2 fails, rate 1.0 >= 0.25");

        // Rate must drop below 0.15 to recover: 2 fails need > 11 successes.
        for _ in 0..11 {
            s.record_success("rs", "map");
        }
        assert!(s.is_risky("rs", "map"), "2/13 ≈ 0.154 still risky");
        s.record_success("rs", "map");
        assert!(!s.is_risky("rs", "map"), "2/14 ≈ 0.143 < 0.15 recovers");
    }

    #[test]
    fn entering_risky_needs_quarter_rate_not_just_two_fails() {
        let mut s = EditQualityStore::default();
        for _ in 0..7 {
            s.record_success("ts", "signatures");
        }
        s.record_failure("ts", "signatures", 1000);
        s.record_failure("ts", "signatures", 1001);
        // 2 fails / 9 total ≈ 0.22 < 0.25 — healthy mode stays usable.
        assert!(!s.is_risky("ts", "signatures"));
        s.record_failure("ts", "signatures", 1002);
        // 3/10 = 0.30 — now risky.
        assert!(s.is_risky("ts", "signatures"));
    }

    #[test]
    fn penalty_is_per_extension_not_global() {
        let mut s = EditQualityStore::default();
        s.record_failure("rs", "map", 1000);
        s.record_failure("rs", "map", 1001);
        assert!(s.is_risky("rs", "map"));
        assert!(!s.is_risky("py", "map"), "py|map untouched");
        assert!(!s.is_risky("rs", "signatures"), "rs|signatures untouched");
    }

    #[test]
    fn escalation_is_one_shot_and_expires() {
        let mut s = EditQualityStore::default();
        s.set_pending_escalation("src/a.rs", 1000);
        assert!(s.take_pending_escalation("src/a.rs", 1100));
        assert!(
            !s.take_pending_escalation("src/a.rs", 1101),
            "consumed — second read is normal again"
        );

        s.set_pending_escalation("src/b.rs", 1000);
        assert!(
            !s.take_pending_escalation("src/b.rs", 1000 + ESCALATION_TTL_SECS + 1),
            "expired escalations are dropped, not served"
        );
    }

    #[test]
    fn anchored_escalation_is_independent_and_one_shot() {
        // The anchored map is separate from the `full` map — arming one
        // must never consume the other, so str_replace and sobre_patch recoveries
        // don't cross-talk.
        let mut s = EditQualityStore::default();
        s.set_pending_anchored_escalation("src/a.rs", 1000);
        assert!(
            !s.take_pending_escalation("src/a.rs", 1100),
            "anchored arming must not satisfy a full escalation"
        );
        assert!(s.take_pending_anchored_escalation("src/a.rs", 1100));
        assert!(
            !s.take_pending_anchored_escalation("src/a.rs", 1101),
            "anchored escalation is one-shot"
        );
    }

    #[test]
    fn anchored_outcome_arms_anchored_not_full() {
        // A miss after an anchored read arms only the anchored escalation.
        let mut s = EditQualityStore::default();
        s.record_failure("rs", "anchored", 1000);
        s.set_pending_anchored_escalation("src/x.rs", 1000);
        assert!(s.pending_escalations.is_empty());
        assert_eq!(s.pending_anchored_escalations.len(), 1);
    }

    #[test]
    fn eviction_keeps_newest() {
        let mut s = EditQualityStore::default();
        for i in 0..(MAX_PAIRS + 10) {
            s.record_failure(&format!("e{i}"), "map", 1000 + i as u64);
        }
        assert_eq!(s.pairs.len(), MAX_PAIRS);
        assert!(!s.pairs.contains_key("e0|map"));
        for i in 0..(MAX_PENDING + 5) {
            s.set_pending_escalation(&format!("f{i}.rs"), 1000 + i as u64);
        }
        assert_eq!(s.pending_escalations.len(), MAX_PENDING);
        assert!(!s.pending_escalations.contains_key("f0.rs"));
    }
}
