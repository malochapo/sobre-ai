use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const MAX_SPANS: usize = 2048;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum SpanStatus {
    Ok,
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OclaSpan {
    pub span_id: String,
    pub trace_id: String,
    pub parent_span_id: Option<String>,
    pub operation: String,
    pub start_ns: u64,
    pub end_ns: Option<u64>,
    pub status: SpanStatus,
    pub attributes: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct SpanCollector {
    spans: Arc<Mutex<VecDeque<OclaSpan>>>,
}

impl SpanCollector {
    pub fn new() -> Self {
        Self {
            spans: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_SPANS))),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<OclaSpan>> {
        self.spans.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(&self, span: OclaSpan) {
        let mut spans = self.lock();
        if spans.len() == MAX_SPANS {
            spans.pop_front();
        }
        spans.push_back(span);
    }

    fn finish(&self, span_id: &str, end_ns: u64) {
        let mut spans = self.lock();
        if let Some(span) = spans.iter_mut().find(|span| span.span_id == span_id) {
            span.end_ns = Some(end_ns);
        }
    }

    fn set_status(&self, span_id: &str, status: SpanStatus) {
        let mut spans = self.lock();
        if let Some(span) = spans.iter_mut().find(|span| span.span_id == span_id) {
            span.status = status;
        }
    }

    fn add_attribute(&self, span_id: &str, key: String, value: String) {
        let mut spans = self.lock();
        if let Some(span) = spans.iter_mut().find(|span| span.span_id == span_id) {
            span.attributes.push((key, value));
        }
    }

    fn spans_for_trace(&self, trace_id: &str) -> Vec<OclaSpan> {
        let spans = self.lock();
        spans
            .iter()
            .filter(|span| span.trace_id == trace_id)
            .cloned()
            .collect()
    }
}

struct ActiveSpan {
    trace_id: String,
    span_id: String,
}

thread_local! {
    static ACTIVE_SPANS: std::cell::RefCell<Vec<ActiveSpan>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

static COLLECTOR: OnceLock<SpanCollector> = OnceLock::new();

fn collector() -> &'static SpanCollector {
    COLLECTOR.get_or_init(SpanCollector::new)
}

fn next_span_id() -> String {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).expect("CSPRNG unavailable");
    format!("{:016x}", u64::from_be_bytes(bytes))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// RAII handle that closes its span when dropped.
pub struct SpanGuard {
    collector: &'static SpanCollector,
    span_id: String,
}

impl SpanGuard {
    pub fn set_status(&self, status: SpanStatus) {
        self.collector.set_status(&self.span_id, status);
    }

    pub fn add_attribute(&self, key: impl Into<String>, value: impl Into<String>) {
        self.collector
            .add_attribute(&self.span_id, key.into(), value.into());
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        self.collector.finish(&self.span_id, now_ns());
        ACTIVE_SPANS.with(|active| {
            let mut active = active.borrow_mut();
            if let Some(index) = active.iter().rposition(|span| span.span_id == self.span_id) {
                active.remove(index);
            }
        });
    }
}

pub fn start_span(trace_id: &str, operation: &str) -> SpanGuard {
    let span_id = next_span_id();
    let parent_span_id = ACTIVE_SPANS.with(|active| {
        active
            .borrow()
            .iter()
            .rev()
            .find(|span| span.trace_id == trace_id)
            .map(|span| span.span_id.clone())
    });
    collector().register(OclaSpan {
        span_id: span_id.clone(),
        trace_id: trace_id.to_string(),
        parent_span_id,
        operation: operation.to_string(),
        start_ns: now_ns(),
        end_ns: None,
        status: SpanStatus::Ok,
        attributes: Vec::new(),
    });
    ACTIVE_SPANS.with(|active| {
        active.borrow_mut().push(ActiveSpan {
            trace_id: trace_id.to_string(),
            span_id: span_id.clone(),
        });
    });
    SpanGuard {
        collector: collector(),
        span_id,
    }
}

pub fn spans_for_trace(trace_id: &str) -> Vec<OclaSpan> {
    collector().spans_for_trace(trace_id)
}

fn status_value(status: &SpanStatus) -> serde_json::Value {
    match status {
        SpanStatus::Ok => serde_json::json!({"code": "STATUS_OK"}),
        SpanStatus::Error(message) => {
            serde_json::json!({"code": "STATUS_ERROR", "message": message})
        }
    }
}

pub fn export_trace(trace_id: &str) -> serde_json::Value {
    let spans: Vec<serde_json::Value> = spans_for_trace(trace_id)
        .into_iter()
        .map(|span| {
            let attributes: Vec<serde_json::Value> = span
                .attributes
                .into_iter()
                .map(|(key, value)| {
                    serde_json::json!({
                        "key": key,
                        "value": {"stringValue": value}
                    })
                })
                .collect();
            let mut exported = serde_json::json!({
                "traceId": span.trace_id,
                "spanId": span.span_id,
                "name": span.operation,
                "startTimeUnixNano": span.start_ns.to_string(),
                "status": status_value(&span.status),
                "attributes": attributes,
            });
            if let Some(parent_span_id) = span.parent_span_id {
                exported["parentSpanId"] = serde_json::Value::String(parent_span_id);
            }
            if let Some(end_ns) = span.end_ns {
                exported["endTimeUnixNano"] = serde_json::Value::String(end_ns.to_string());
            }
            exported
        })
        .collect();
    serde_json::json!({
        "resourceSpans": [{
            "scopeSpans": [{"spans": spans}]
        }]
    })
}

impl Default for SpanCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_trace(label: &str) -> String {
        format!("{label}-{}", next_span_id())
    }

    #[test]
    fn span_closes_with_monotonic_wall_clock_timing() {
        let trace_id = unique_trace("timing");
        drop(start_span(&trace_id, "test.operation"));
        let span = spans_for_trace(&trace_id).pop().expect("span retained");
        assert!(span.end_ns.expect("end time") >= span.start_ns);
    }

    #[test]
    fn nested_spans_link_to_the_active_parent() {
        let trace_id = unique_trace("parent");
        let parent = start_span(&trace_id, "parent");
        let parent_id = parent.span_id.clone();
        let child = start_span(&trace_id, "child");
        let child_id = child.span_id.clone();
        drop(child);
        drop(parent);
        let child = spans_for_trace(&trace_id)
            .into_iter()
            .find(|span| span.span_id == child_id)
            .expect("child retained");
        assert_eq!(child.parent_span_id.as_deref(), Some(parent_id.as_str()));
    }

    #[test]
    fn traces_are_grouped_and_exported_as_otel_json() {
        let trace_id = unique_trace("export");
        let guard = start_span(&trace_id, "export.operation");
        guard.set_status(SpanStatus::Error("failed".into()));
        guard.add_attribute("component", "ocla");
        drop(guard);
        assert_eq!(spans_for_trace(&trace_id).len(), 1);
        let exported = export_trace(&trace_id);
        let span = &exported["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span["name"], "export.operation");
        assert_eq!(span["status"]["code"], "STATUS_ERROR");
        assert_eq!(span["attributes"][0]["value"]["stringValue"], "ocla");
    }

    #[test]
    fn collector_evicts_oldest_spans_at_capacity() {
        let collector = SpanCollector::new();
        for index in 0..=MAX_SPANS {
            collector.register(OclaSpan {
                span_id: index.to_string(),
                trace_id: "overflow".into(),
                parent_span_id: None,
                operation: "test".into(),
                start_ns: index as u64,
                end_ns: Some(index as u64),
                status: SpanStatus::Ok,
                attributes: Vec::new(),
            });
        }
        let spans = collector.spans_for_trace("overflow");
        assert_eq!(spans[0].span_id, "1");
        assert_eq!(spans[MAX_SPANS - 1].span_id, MAX_SPANS.to_string());
    }
}
