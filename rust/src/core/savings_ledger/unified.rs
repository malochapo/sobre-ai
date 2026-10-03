use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom, Write};
use std::path::PathBuf;

use fs2::FileExt;

use crate::core::request_context::{ContextError, ContextResult, RequestContext};
use crate::core::savings_ledger::SavingsEvent;

/// Unified P5 savings event combining the legacy chain fields with
/// cross-capability attribution and analysis metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnifiedSavingsEventV2 {
    pub tool_name: String,
    pub mode: String,
    pub original_tokens: u64,
    pub compressed_tokens: u64,
    pub saved_tokens: u64,
    pub content_hash: String,
    pub timestamp_epoch_ms: u64,
    pub prev_hash: String,
    pub event_hash: String,
    pub intent: Option<String>,
    pub outcome: Option<String>,
    pub routing_decision: Option<String>,
    pub agent_id: Option<String>,
    pub efficiency_etpao: Option<u64>,
    pub attribution_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

/// Comparison of the legacy and unified savings ledgers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReconciliationReport {
    pub matched: usize,
    pub unmatched_legacy: usize,
    pub unmatched_unified: usize,
    pub token_drift: i64,
    pub double_bookings: Vec<String>,
}

/// Unified ledger contract for P5 migration and eventual legacy replacement.
///
/// Migration plan:
/// - Phase 1: introduce this schema alongside the legacy schema (dual-write).
/// - Phase 2: migrate existing events into unified events.
/// - Phase 3: deactivate the legacy schema after migration verification.
pub trait UnifiedLedger: Send + Sync {
    fn record_unified(&self, event: UnifiedSavingsEventV2) -> ContextResult<String>;
    fn verify_chain(&self) -> ContextResult<bool>;
    fn query_by_attribution(&self, id: &str) -> ContextResult<Option<UnifiedSavingsEventV2>>;
}

/// File-backed implementation used during the P5 dual-write migration.
pub(crate) struct FileUnifiedLedger {
    path: PathBuf,
}

impl FileUnifiedLedger {
    pub(crate) fn from_data_dir() -> ContextResult<Self> {
        let data_dir = crate::core::data_dir::lean_ctx_data_dir()
            .map_err(|error| ContextError::InvalidRequest(error.clone()))?;
        Ok(Self::new(data_dir.join("savings/unified_ledger.jsonl")))
    }

    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn io_error(error: impl std::fmt::Display) -> ContextError {
        ContextError::InvalidRequest(format!("unified ledger I/O failed: {error}"))
    }

    fn read_events(&self) -> ContextResult<Vec<UnifiedSavingsEventV2>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Self::io_error(error)),
        };
        file.lock_shared().map_err(Self::io_error)?;
        let result = BufReader::new(&file)
            .lines()
            .map(|line| {
                let line = line.map_err(Self::io_error)?;
                serde_json::from_str(&line).map_err(Self::io_error)
            })
            .collect();
        let _ = file.unlock();
        result
    }

    fn read_legacy_events(&self) -> Vec<SavingsEvent> {
        let events_path = self.path.with_file_name("events.jsonl");
        if events_path.exists() {
            crate::core::savings_ledger::store::load(&events_path)
        } else {
            crate::core::savings_ledger::store::load(&self.path.with_file_name("ledger.jsonl"))
        }
    }

    /// Compares legacy and unified entries by hash and reports accounting drift.
    pub(crate) fn reconcile(&self) -> ContextResult<ReconciliationReport> {
        let legacy = self.read_legacy_events();
        let unified = self.read_events()?;
        let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();

        let legacy_tokens: u64 = legacy.iter().map(|event| event.saved_tokens).sum();
        for event in &legacy {
            counts.entry(event.entry_hash.clone()).or_default().0 += 1;
        }

        let unified_tokens: u64 = unified.iter().map(|event| event.saved_tokens).sum();
        for event in &unified {
            counts.entry(event.event_hash.clone()).or_default().1 += 1;
        }

        let matched = counts
            .values()
            .map(|(legacy_count, unified_count)| (*legacy_count).min(*unified_count))
            .sum();
        let double_bookings = counts
            .into_iter()
            .filter_map(|(hash, (legacy_count, unified_count))| {
                (legacy_count > 1 || unified_count > 1).then_some(hash)
            })
            .collect();

        let token_delta = i128::from(unified_tokens) - i128::from(legacy_tokens);
        let token_drift = i64::try_from(token_delta).unwrap_or_else(|_| {
            if token_delta.is_negative() {
                i64::MIN
            } else {
                i64::MAX
            }
        });

        Ok(ReconciliationReport {
            matched,
            unmatched_legacy: legacy.len() - matched,
            unmatched_unified: unified.len() - matched,
            token_drift,
            double_bookings,
        })
    }

    pub(crate) fn from_savings_event(event: &SavingsEvent) -> ContextResult<UnifiedSavingsEventV2> {
        let timestamp_epoch_ms = chrono::DateTime::parse_from_rfc3339(&event.ts)
            .map_err(Self::io_error)?
            .timestamp_millis();
        let timestamp_epoch_ms = u64::try_from(timestamp_epoch_ms)
            .map_err(|error| Self::io_error(format!("invalid event timestamp: {error}")))?;

        Ok(UnifiedSavingsEventV2 {
            tool_name: event.tool.clone(),
            mode: event.mechanism.clone(),
            original_tokens: event.baseline_tokens,
            compressed_tokens: event.actual_tokens,
            saved_tokens: event.saved_tokens,
            content_hash: event.repo_hash.clone(),
            timestamp_epoch_ms,
            prev_hash: event.prev_hash.clone(),
            event_hash: event.entry_hash.clone(),
            intent: event.intent_tag.clone(),
            outcome: event.outcome.clone(),
            routing_decision: event.model_routed.clone(),
            agent_id: Some(event.agent_id.clone()),
            efficiency_etpao: None,
            attribution_id: event
                .attribution_id
                .clone()
                .unwrap_or_else(|| event.repo_hash.clone()),
            trace_id: RequestContext::current_trace_id(),
        })
    }
}

impl UnifiedLedger for FileUnifiedLedger {
    fn record_unified(&self, event: UnifiedSavingsEventV2) -> ContextResult<String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(Self::io_error)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .map_err(Self::io_error)?;
        file.lock_exclusive().map_err(Self::io_error)?;
        let result = (|| {
            file.seek(SeekFrom::Start(0)).map_err(Self::io_error)?;
            let mut last_hash = None;
            for line in BufReader::new(&file).lines() {
                let line = line.map_err(Self::io_error)?;
                let previous: UnifiedSavingsEventV2 =
                    serde_json::from_str(&line).map_err(Self::io_error)?;
                last_hash = Some(previous.event_hash);
            }
            if event.prev_hash != last_hash.as_deref().unwrap_or("genesis") {
                return Err(ContextError::InvalidRequest(
                    "unified ledger chain link mismatch".into(),
                ));
            }
            let line = serde_json::to_string(&event).map_err(Self::io_error)?;
            file.seek(SeekFrom::End(0)).map_err(Self::io_error)?;
            writeln!(file, "{line}").map_err(Self::io_error)?;
            Ok(event.event_hash.clone())
        })();
        let _ = file.unlock();
        result
    }

    fn verify_chain(&self) -> ContextResult<bool> {
        let events = self.read_events()?;
        Ok(events.iter().enumerate().all(|(index, event)| {
            event.prev_hash
                == if index == 0 {
                    "genesis"
                } else {
                    events[index - 1].event_hash.as_str()
                }
        }))
    }

    fn query_by_attribution(&self, id: &str) -> ContextResult<Option<UnifiedSavingsEventV2>> {
        Ok(self
            .read_events()?
            .into_iter()
            .find(|event| event.attribution_id == id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn savings_event() -> SavingsEvent {
        SavingsEvent {
            ts: "2026-01-01T00:00:00Z".into(),
            tool: "ctx_read".into(),
            mechanism: "compression".into(),
            model_id: "model".into(),
            tokenizer: "tokenizer".into(),
            baseline_tokens: 100,
            actual_tokens: 40,
            saved_tokens: 60,
            bounce_adjustment: 0,
            unit_price_per_m_usd: 1.0,
            saved_usd: 0.00006,
            repo_hash: "repo".into(),
            agent_id: "agent".into(),
            prev_hash: "genesis".into(),
            entry_hash: "event".into(),
            version: "version".into(),
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
            customer_approval: None,
            settlement_status: None,
            is_first_inject: None,
            cache_read_per_m_usd: None,
            cache_write_per_m_usd: None,
        }
    }

    #[test]
    fn request_context_trace_id_reaches_unified_event() {
        let context = RequestContext {
            request_id: "request".into(),
            session_id: "session".into(),
            agent_id: "agent".into(),
            content_ref: "content".into(),
            tenant_id: None,
            trace_id: "tr-request".into(),
        };
        let unified = context.scope(|| {
            FileUnifiedLedger::from_savings_event(&savings_event()).expect("legacy event converts")
        });
        assert_eq!(unified.trace_id.as_deref(), Some("tr-request"));
    }

    #[test]
    fn file_ledger_records_verifies_and_queries_events() {
        let path = std::env::temp_dir().join(format!(
            "lean-ctx-unified-ledger-{}.jsonl",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let ledger = FileUnifiedLedger::new(path.clone());
        let event = UnifiedSavingsEventV2 {
            tool_name: "ctx_read".into(),
            mode: "compression".into(),
            original_tokens: 100,
            compressed_tokens: 40,
            saved_tokens: 60,
            content_hash: "repo".into(),
            timestamp_epoch_ms: 1,
            prev_hash: "genesis".into(),
            event_hash: "event-1".into(),
            intent: None,
            outcome: None,
            routing_decision: None,
            agent_id: Some("agent".into()),
            efficiency_etpao: None,
            attribution_id: "attr".into(),
            trace_id: None,
        };
        assert_eq!(ledger.record_unified(event).unwrap(), "event-1");
        assert!(ledger.verify_chain().unwrap());
        assert_eq!(
            ledger
                .query_by_attribution("attr")
                .unwrap()
                .unwrap()
                .saved_tokens,
            60
        );
        let _ = fs::remove_file(path);
    }

    fn legacy_event(saved_tokens: u64) -> SavingsEvent {
        serde_json::from_value(serde_json::json!({
            "ts": "2026-06-01T00:00:00+00:00",
            "tool": "ctx_read",
            "mechanism": "compression",
            "model_id": "test-model",
            "tokenizer": "o200k_base",
            "baseline_tokens": 100,
            "actual_tokens": 100 - saved_tokens,
            "saved_tokens": saved_tokens,
            "bounce_adjustment": 0,
            "unit_price_per_m_usd": 2.5,
            "saved_usd": saved_tokens as f64 * 2.5 / 1_000_000.0,
            "repo_hash": "repo",
            "agent_id": "agent",
            "prev_hash": "",
            "entry_hash": "",
            "version": "test"
        }))
        .unwrap()
    }

    #[test]
    fn reconcile_matches_legacy_and_unified_events() {
        let dir = tempfile::tempdir().unwrap();
        let savings = dir.path().join("savings");
        let legacy_path = savings.join("events.jsonl");
        let unified_path = savings.join("unified_ledger.jsonl");
        let legacy =
            crate::core::savings_ledger::store::append(&legacy_path, legacy_event(60)).unwrap();
        let ledger = FileUnifiedLedger::new(unified_path);
        ledger
            .record_unified(FileUnifiedLedger::from_savings_event(&legacy).unwrap())
            .unwrap();

        assert_eq!(
            ledger.reconcile().unwrap(),
            ReconciliationReport {
                matched: 1,
                unmatched_legacy: 0,
                unmatched_unified: 0,
                token_drift: 0,
                double_bookings: Vec::new(),
            }
        );
    }

    #[test]
    fn reconcile_reports_drift_and_double_bookings() {
        let dir = tempfile::tempdir().unwrap();
        let savings = dir.path().join("savings");
        let legacy_path = savings.join("events.jsonl");
        let unified_path = savings.join("unified_ledger.jsonl");
        let legacy =
            crate::core::savings_ledger::store::append(&legacy_path, legacy_event(60)).unwrap();
        let ledger = FileUnifiedLedger::new(unified_path);
        let unified = FileUnifiedLedger::from_savings_event(&legacy).unwrap();
        ledger.record_unified(unified.clone()).unwrap();
        let mut duplicate = unified;
        duplicate.prev_hash = duplicate.event_hash.clone();
        ledger.record_unified(duplicate).unwrap();

        let report = ledger.reconcile().unwrap();
        assert_eq!(report.matched, 1);
        assert_eq!(report.unmatched_legacy, 0);
        assert_eq!(report.unmatched_unified, 1);
        assert_eq!(report.token_drift, 60);
        assert_eq!(report.double_bookings, vec![legacy.entry_hash]);
    }
}
