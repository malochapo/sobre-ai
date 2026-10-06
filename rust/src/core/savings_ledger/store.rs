//! Append-only, SHA-256 hash-chained JSONL store for [`SavingsEvent`]s.
//!
//! Appends are serialised across processes with an advisory file lock (`fs2`), so the
//! MCP server and CLI can both write to one correct chain. The last hash is read from
//! the file tail under the lock (O(1) per append), not cached, to stay correct under
//! concurrent writers. Cryptographic signing of batches is a later phase (G5/G6).

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::event::{EvidenceClass, MeasurementMethod, SavingsEvent, compute_hash};

pub const GENESIS: &str = "genesis";
const TAIL_READ_BYTES: u64 = 8192;

/// Default ledger location: `<data_dir>/savings/ledger.jsonl`. Local only.
pub fn default_path() -> Option<PathBuf> {
    let dir = crate::core::data_dir::lean_ctx_data_dir().ok()?;
    let sub = dir.join("savings");
    fs::create_dir_all(&sub).ok()?;
    Some(sub.join("ledger.jsonl"))
}

/// Reads the most recent `entry_hash` by scanning only the file tail. Returns
/// [`GENESIS`] for an empty/new file.
fn read_last_hash_from_tail(file: &mut fs::File) -> std::io::Result<String> {
    let len = file.seek(SeekFrom::End(0))?;
    if len == 0 {
        return Ok(GENESIS.to_string());
    }
    let read_size = len.min(TAIL_READ_BYTES);
    file.seek(SeekFrom::End(-(read_size as i64)))?;
    let mut buf = vec![0u8; read_size as usize];
    file.read_exact(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().rev() {
        if let Ok(ev) = serde_json::from_str::<SavingsEvent>(line) {
            return Ok(ev.entry_hash);
        }
    }
    Ok(GENESIS.to_string())
}

/// Appends one event, filling `prev_hash`/`entry_hash` under an exclusive lock.
/// Returns the finalised event. Best-effort on serialise failure (no write, no error).
pub fn append(path: &Path, mut ev: SavingsEvent) -> std::io::Result<SavingsEvent> {
    if ev.measurement_method.is_none() {
        ev.measurement_method = Some(match ev.mechanism.as_str() {
            "compression" => MeasurementMethod::DirectCount,
            "routing" => MeasurementMethod::BaselineEstimate,
            "caching" => MeasurementMethod::ProviderReconciled,
            _ => MeasurementMethod::Unknown,
        });
    }
    if ev.evidence_class.is_none() {
        ev.evidence_class = Some(match ev.mechanism.as_str() {
            "compression" | "caching" => EvidenceClass::Measured,
            "routing" => EvidenceClass::Approximated,
            _ => EvidenceClass::Unclassified,
        });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    file.lock_exclusive()?;
    let result = append_locked(&mut file, &mut ev);
    let _ = FileExt::unlock(&file);
    result.map(|()| ev)
}

fn append_locked(file: &mut fs::File, ev: &mut SavingsEvent) -> std::io::Result<()> {
    let prev = read_last_hash_from_tail(file)?;
    ev.entry_hash = compute_hash(&prev, &ev.canonical_content());
    ev.prev_hash = prev;
    if let Ok(line) = serde_json::to_string(ev) {
        file.seek(SeekFrom::End(0))?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

/// Loads every event (whole file). Callers that only need totals should prefer
/// [`summarize`], which streams.
pub fn load(path: &Path) -> Vec<SavingsEvent> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct LedgerSummary {
    pub total_events: usize,
    /// Gross saved tokens (read events only; bounce events do not inflate this).
    pub saved_tokens: u64,
    /// Net USD: read savings minus bounce events (which carry negative `saved_usd`).
    pub saved_usd: f64,
    /// Tokens later wasted by a compressed->full re-read (sum of `bounce_adjustment`).
    pub bounce_tokens: u64,
    /// Number of recorded bounce events.
    pub bounce_events: usize,
    /// Distinct tokenizers that produced the recorded counts (usually just `o200k_base`).
    pub tokenizers: Vec<String>,
    /// (model_id, saved_tokens, saved_usd), descending by tokens.
    pub by_model: Vec<(String, u64, f64)>,
    /// (YYYY-MM-DD, saved_tokens, saved_usd), ascending by day.
    pub by_day: Vec<(String, u64, f64)>,
    /// (tool, saved_tokens), descending by tokens.
    pub by_tool: Vec<(String, u64)>,
    /// (mechanism, saved_tokens, saved_usd), descending by USD — the
    /// attribution slice (enterprise#19): compression | routing | caching.
    pub by_mechanism: Vec<(String, u64, f64)>,
    /// G8 Token-Stream USD Attribution (#1191): (stream_name, saved_tokens, saved_usd).
    /// Streams: "first_inject" (cache_write rate) and "re_read" (cache_read rate).
    pub by_stream: Vec<(String, u64, f64)>,
}

impl LedgerSummary {
    /// Net saved tokens = gross savings minus bounce.
    #[cfg(test)]
    pub fn net_saved_tokens(&self) -> u64 {
        self.saved_tokens.saturating_sub(self.bounce_tokens)
    }
}

/// Streams the ledger and aggregates totals sliceable by model / day / tool.
pub fn summarize(path: &Path) -> LedgerSummary {
    use std::collections::HashMap;
    let Ok(file) = fs::File::open(path) else {
        return LedgerSummary::default();
    };

    let mut s = LedgerSummary::default();
    let mut by_model: HashMap<String, (u64, f64)> = HashMap::new();
    let mut by_day: HashMap<String, (u64, f64)> = HashMap::new();
    let mut by_tool: HashMap<String, u64> = HashMap::new();
    let mut by_mechanism: HashMap<String, (u64, f64)> = HashMap::new();
    let mut tokenizers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut first_inject: (u64, f64) = (0, 0.0);
    let mut re_read: (u64, f64) = (0, 0.0);

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(ev) = serde_json::from_str::<SavingsEvent>(&line) else {
            continue;
        };
        s.total_events += 1;
        s.saved_tokens = s.saved_tokens.saturating_add(ev.saved_tokens);
        s.saved_usd += ev.saved_usd;
        s.bounce_tokens = s.bounce_tokens.saturating_add(ev.bounce_adjustment);
        if ev.bounce_adjustment > 0 {
            s.bounce_events += 1;
        }
        if !ev.tokenizer.is_empty() {
            tokenizers.insert(ev.tokenizer.clone());
        }

        // Attribution slice (enterprise#19): value-bearing events by mechanism.
        // Routing/caching events carry USD at zero saved_tokens, so the filter
        // is on value, not tokens; bounces stay out (they net the headline).
        if ev.saved_tokens > 0 || (ev.saved_usd != 0.0 && ev.bounce_adjustment == 0) {
            let mech = by_mechanism.entry(ev.mechanism.clone()).or_default();
            mech.0 = mech.0.saturating_add(ev.saved_tokens);
            mech.1 += ev.saved_usd;
        }

        // Breakdowns describe *savings* — bounce events (saved_tokens == 0, negative USD)
        // are netted into the headline totals above but kept out of the slices below.
        if ev.saved_tokens > 0 {
            let m = by_model.entry(ev.model_id.clone()).or_default();
            m.0 = m.0.saturating_add(ev.saved_tokens);
            m.1 += ev.saved_usd;

            let day = ev.ts.get(..10).unwrap_or("").to_string();
            let d = by_day.entry(day).or_default();
            d.0 = d.0.saturating_add(ev.saved_tokens);
            d.1 += ev.saved_usd;

            *by_tool.entry(ev.tool.clone()).or_default() += ev.saved_tokens;
        }

        // G8 Token-Stream Attribution (#1191): classify by first-inject vs re-read.
        // For events with the G8 field, use it directly. For pre-G8 events,
        // estimate based on cache_read rate vs input rate.
        if ev.saved_tokens > 0 {
            let is_first = ev.is_first_inject.unwrap_or(true);
            let stream_usd = if is_first {
                let rate = ev
                    .cache_write_per_m_usd
                    .unwrap_or(ev.unit_price_per_m_usd * 1.25);
                ev.saved_tokens as f64 / 1_000_000.0 * rate
            } else {
                let rate = ev
                    .cache_read_per_m_usd
                    .unwrap_or(ev.unit_price_per_m_usd * 0.1);
                ev.saved_tokens as f64 / 1_000_000.0 * rate
            };
            if is_first {
                first_inject.0 = first_inject.0.saturating_add(ev.saved_tokens);
                first_inject.1 += stream_usd;
            } else {
                re_read.0 = re_read.0.saturating_add(ev.saved_tokens);
                re_read.1 += stream_usd;
            }
        }
    }

    s.by_model = by_model.into_iter().map(|(k, (t, u))| (k, t, u)).collect();
    s.by_model.sort_by_key(|row| std::cmp::Reverse(row.1));

    s.by_day = by_day.into_iter().map(|(k, (t, u))| (k, t, u)).collect();
    s.by_day.sort_by(|a, b| a.0.cmp(&b.0));

    s.by_tool = by_tool.into_iter().collect();
    s.by_tool.sort_by_key(|row| std::cmp::Reverse(row.1));

    s.by_mechanism = by_mechanism
        .into_iter()
        .map(|(k, (t, u))| (k, t, u))
        .collect();
    s.by_mechanism
        .sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    s.tokenizers = tokenizers.into_iter().collect();

    // G8 Token-Stream Attribution (#1191)
    let mut streams = Vec::new();
    if first_inject.0 > 0 {
        streams.push(("first_inject".to_string(), first_inject.0, first_inject.1));
    }
    if re_read.0 > 0 {
        streams.push(("re_read".to_string(), re_read.0, re_read.1));
    }
    s.by_stream = streams;
    s
}

/// Sums `bounce_adjustment` over the ledger, optionally limited to events within the last
/// `days` (by RFC3339 timestamp). `None` = all time.
pub fn bounce_tokens_since(path: &Path, days: Option<u32>) -> u64 {
    let Ok(file) = fs::File::open(path) else {
        return 0;
    };
    let cutoff = days.map(|d| chrono::Utc::now() - chrono::Duration::days(i64::from(d)));
    let mut total = 0u64;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(ev) = serde_json::from_str::<SavingsEvent>(&line) else {
            continue;
        };
        if ev.bounce_adjustment == 0 {
            continue;
        }
        if let Some(cut) = cutoff {
            match chrono::DateTime::parse_from_rfc3339(&ev.ts) {
                Ok(t) if t.with_timezone(&chrono::Utc) < cut => continue,
                _ => {}
            }
        }
        total = total.saturating_add(ev.bounce_adjustment);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::super::event::MECHANISM_COMPRESSION;
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!(
            "lean-ctx-ledger-{}-{}-{nanos}.jsonl",
            std::process::id(),
            tag
        ))
    }

    fn sample(saved: u64) -> SavingsEvent {
        SavingsEvent {
            ts: "2026-06-01T12:00:00+00:00".into(),
            tool: "ctx_read".into(),
            mechanism: MECHANISM_COMPRESSION.into(),
            model_id: "claude-3.5-sonnet".into(),
            tokenizer: "o200k_base".into(),
            baseline_tokens: saved + 100,
            actual_tokens: 100,
            saved_tokens: saved,
            bounce_adjustment: 0,
            unit_price_per_m_usd: 3.0,
            saved_usd: saved as f64 / 1_000_000.0 * 3.0,
            repo_hash: "repo".into(),
            agent_id: "local".into(),
            prev_hash: String::new(),
            entry_hash: String::new(),
            version: "3.9.0".into(),
            intent_tag: None,
            outcome: None,
            model_original: None,
            model_routed: None,
            routing_savings: None,
            response_original_tokens: None,
            response_delivered_tokens: None,
            agent_chain_id: None,
            chain_depth: None,
            measurement_method: None,
            evidence_class: None,
            confidence: None,
            quality_signal: None,
            attribution_group: None,
            attribution_id: None,
            baseline_ref: None,
            price_version: None,
            is_first_inject: None,
            cache_read_per_m_usd: None,
            cache_write_per_m_usd: None,
        }
    }

    #[test]
    fn append_fills_p5_evidence_fields_from_mechanism() {
        let p = temp_path("evidence-fallback");
        let cases = [
            (
                "compression",
                MeasurementMethod::DirectCount,
                EvidenceClass::Measured,
            ),
            (
                "routing",
                MeasurementMethod::BaselineEstimate,
                EvidenceClass::Approximated,
            ),
            (
                "caching",
                MeasurementMethod::ProviderReconciled,
                EvidenceClass::Measured,
            ),
            (
                "other",
                MeasurementMethod::Unknown,
                EvidenceClass::Unclassified,
            ),
        ];

        for (mechanism, method, evidence) in cases {
            let mut event = sample(1);
            event.mechanism = mechanism.to_string();
            let appended = append(&p, event).unwrap();
            assert_eq!(appended.measurement_method, Some(method));
            assert_eq!(appended.evidence_class, Some(evidence));
        }

        let mut explicit = sample(1);
        explicit.measurement_method = Some(MeasurementMethod::Holdout);
        explicit.evidence_class = Some(EvidenceClass::Statistical);
        let appended = append(&p, explicit).unwrap();
        assert_eq!(
            appended.measurement_method,
            Some(MeasurementMethod::Holdout)
        );
        assert_eq!(appended.evidence_class, Some(EvidenceClass::Statistical));

        let _ = fs::remove_file(&p);
    }

    #[test]
    fn summarize_aggregates_totals_and_slices() {
        let p = temp_path("sum");
        append(&p, sample(500)).unwrap();
        append(&p, sample(300)).unwrap();

        let s = summarize(&p);
        assert_eq!(s.total_events, 2);
        assert_eq!(s.saved_tokens, 800);
        assert!((s.saved_usd - 800.0 / 1_000_000.0 * 3.0).abs() < 1e-9);
        assert_eq!(s.by_model.len(), 1);
        assert_eq!(s.by_model[0].1, 800);
        assert_eq!(s.by_tool[0], ("ctx_read".to_string(), 800));
        assert_eq!(s.by_mechanism.len(), 1);
        assert_eq!(s.by_mechanism[0].0, MECHANISM_COMPRESSION);
        assert_eq!(s.by_mechanism[0].1, 800);

        let _ = fs::remove_file(&p);
    }
}
