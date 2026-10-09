//! Deterministic JSON crusher: structural JSON compaction (a port of
//! Headroom's "Smart Crusher").
//!
//! Real JSON payloads (API responses, `kubectl get -o json`, DB dumps) are
//! dominated by arrays of objects that repeat the same keys and values on
//! every row. [`crush_lossless`] factors that redundancy out: every key present
//! in *all* objects of an array is hoisted to its dominant value (a
//! `_defaults` block), and each item keeps only the fields that *deviate* from
//! the default. A field absent from an item means "equals the default", so the
//! transform is exactly reconstructible (`reconstruct`, test-only).
//!
//! Determinism: the output is a pure function of the input `Value` — no
//! timestamps, counters, randomness, or hash-map order leakage (candidate keys
//! are walked through a [`BTreeSet`], value frequencies through a [`BTreeMap`]).
//! A no-op input returns [`None`], and callers keep the result only when it at
//! least halves the input ([`KEEP_DATA_DIVISOR`]).

use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Marks an object that encodes a crushed array. Chosen to be vanishingly
/// unlikely in real data; if the input already contains this key anywhere, the
/// crusher bails (returns `None`) so reconstruction can never be ambiguous.
const MARKER: &str = "_lc_crush";
const MARKER_ARRAY: &str = "arr";
const DEFAULTS_KEY: &str = "_defaults";
const ITEMS_KEY: &str = "_items";

/// Below this item count, factoring rarely beats its own structural overhead.
const MIN_ITEMS: usize = 3;
/// A key is only hoisted into `_defaults` when its dominant value covers at
/// least this fraction of items (constants are the 100% case).
const MIN_DOMINANCE: f64 = 0.5;
/// Subtrees deeper than this are left untouched.
const MAX_DEPTH: usize = 64;
/// Arrays larger than this are left untouched (pathology guard).
const MAX_ITEMS: usize = 100_000;

/// Shared "the crush must at least halve the payload" threshold. The crusher
/// keeps every datum, so reshaping only pays when the array is redundant
/// enough that the compact form is at most `1/KEEP_DATA_DIVISOR` of the input;
/// heterogeneous/low-redundancy data falls through to each caller's own path.
/// One gate for the shell (`json_schema`, `curl`) and read (`sobre_read`
/// aggressive) paths.
pub const KEEP_DATA_DIVISOR: usize = 2;

/// Lossless crush: the compact JSON text, or `None` when nothing was factored.
pub fn crush_lossless(value: &Value) -> Option<String> {
    if contains_marker(value) {
        return None;
    }
    let crushed = crush_node(value, 0);
    if !crushed.changed {
        return None;
    }
    serde_json::to_string(&crushed.value).ok()
}

/// Lossless crush of `value`, returning the compact text only when it at least
/// halves `raw_len` ([`KEEP_DATA_DIVISOR`]). The shared gate for callers that
/// already hold a parsed `Value` plus its source length (`json_schema`, `curl`).
pub fn crush_value_if_beneficial(value: &Value, raw_len: usize) -> Option<String> {
    let crushed = crush_lossless(value)?;
    (crushed.len().saturating_mul(KEEP_DATA_DIVISOR) <= raw_len).then_some(crushed)
}

/// Parse `text` as JSON and losslessly crush it, returning the compact form only
/// when it at least halves the input ([`KEEP_DATA_DIVISOR`]). The shared gate for
/// callers that start from raw text (`sobre_read` aggressive). `None` for
/// non-JSON or low-redundancy input — the caller keeps its own path.
pub fn crush_text_if_beneficial(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return None;
    }
    let val: Value = serde_json::from_str(trimmed).ok()?;
    crush_value_if_beneficial(&val, trimmed.len())
}

/// Rebuild a `Value` from crushed text.
#[cfg(test)]
pub fn reconstruct(crushed_text: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(crushed_text).ok()?;
    Some(uncrush_node(&v))
}

struct Crushed {
    value: Value,
    changed: bool,
}

impl Crushed {
    fn unchanged(value: Value) -> Self {
        Self {
            value,
            changed: false,
        }
    }
}

fn crush_node(value: &Value, depth: usize) -> Crushed {
    if depth > MAX_DEPTH {
        return Crushed::unchanged(value.clone());
    }
    match value {
        Value::Array(arr) => crush_array(arr, depth),
        Value::Object(map) => {
            let mut out = Map::new();
            let mut changed = false;
            for (key, val) in map {
                let child = crush_node(val, depth + 1);
                changed |= child.changed;
                out.insert(key.clone(), child.value);
            }
            Crushed {
                value: Value::Object(out),
                changed,
            }
        }
        other => Crushed::unchanged(other.clone()),
    }
}

fn crush_array(arr: &[Value], depth: usize) -> Crushed {
    // Crush each element first, so nested arrays inside items also compact and
    // factoring compares already-normalized values.
    let mut items: Vec<Value> = Vec::with_capacity(arr.len());
    let mut child_changed = false;
    for el in arr {
        let child = crush_node(el, depth + 1);
        child_changed |= child.changed;
        items.push(child.value);
    }

    let factorable =
        items.len() >= MIN_ITEMS && items.len() <= MAX_ITEMS && items.iter().all(Value::is_object);
    if !factorable {
        return Crushed {
            value: Value::Array(items),
            changed: child_changed,
        };
    }

    // Keys present in EVERY item are the only factoring candidates: an omitted
    // key would otherwise be indistinguishable from "equals default".
    let mut candidates: BTreeSet<String> = items[0]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    for item in &items[1..] {
        if let Some(obj) = item.as_object() {
            candidates.retain(|k| obj.contains_key(k));
        }
    }

    let n = items.len();
    let mut defaults = Map::new();
    for key in &candidates {
        let values: Vec<&Value> = items.iter().filter_map(|it| it.get(key)).collect();
        let (dominant, dominant_count) = dominant_value(&values);
        if dominant_count >= min_dominant_count(n) {
            defaults.insert(key.clone(), dominant);
        }
    }

    if defaults.is_empty() {
        return Crushed {
            value: Value::Array(items),
            changed: child_changed,
        };
    }

    let new_items: Vec<Value> = items
        .iter()
        .map(|item| {
            let mut slim = Map::new();
            if let Some(obj) = item.as_object() {
                for (key, val) in obj {
                    if defaults.get(key) != Some(val) {
                        slim.insert(key.clone(), val.clone());
                    }
                }
            }
            Value::Object(slim)
        })
        .collect();

    let mut crushed = Map::new();
    crushed.insert(MARKER.to_string(), Value::String(MARKER_ARRAY.to_string()));
    crushed.insert(DEFAULTS_KEY.to_string(), Value::Object(defaults));
    crushed.insert(ITEMS_KEY.to_string(), Value::Array(new_items));

    Crushed {
        value: Value::Object(crushed),
        changed: true,
    }
}

fn min_dominant_count(n: usize) -> usize {
    (((n as f64) * MIN_DOMINANCE).ceil() as usize).max(2)
}

/// Returns the most frequent value and its count. Frequencies key on the
/// canonical JSON serialization; ties break toward the lexicographically
/// smallest serialization for determinism.
fn dominant_value(values: &[&Value]) -> (Value, usize) {
    let mut freq: BTreeMap<String, (usize, Value)> = BTreeMap::new();
    for v in values {
        let key = serde_json::to_string(v).unwrap_or_default();
        let entry = freq.entry(key).or_insert_with(|| (0, (*v).clone()));
        entry.0 += 1;
    }
    let mut best_count = 0usize;
    let mut best_value = Value::Null;
    for (count, value) in freq.values() {
        if *count > best_count {
            best_count = *count;
            best_value = value.clone();
        }
    }
    (best_value, best_count)
}

fn contains_marker(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key(MARKER) || map.values().any(contains_marker),
        Value::Array(arr) => arr.iter().any(contains_marker),
        _ => false,
    }
}

#[cfg(test)]
fn uncrush_node(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if map.get(MARKER) == Some(&Value::String(MARKER_ARRAY.to_string())) {
                let defaults = map.get(DEFAULTS_KEY).and_then(Value::as_object);
                let items = map.get(ITEMS_KEY).and_then(Value::as_array);
                let mut out = Vec::new();
                if let Some(items) = items {
                    for item in items {
                        let mut full = Map::new();
                        if let Some(defaults) = defaults {
                            for (key, val) in defaults {
                                full.insert(key.clone(), uncrush_node(val));
                            }
                        }
                        if let Some(obj) = item.as_object() {
                            for (key, val) in obj {
                                full.insert(key.clone(), uncrush_node(val));
                            }
                        }
                        out.push(Value::Object(full));
                    }
                }
                Value::Array(out)
            } else {
                let mut out = Map::new();
                for (key, val) in map {
                    out.insert(key.clone(), uncrush_node(val));
                }
                Value::Object(out)
            }
        }
        Value::Array(arr) => Value::Array(arr.iter().map(uncrush_node).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn homogeneous() -> Value {
        json!([
            {"status": "success", "region": "eu", "id": 1},
            {"status": "success", "region": "eu", "id": 2},
            {"status": "success", "region": "eu", "id": 3},
            {"status": "success", "region": "eu", "id": 4}
        ])
    }

    #[test]
    fn lossless_factors_constant_columns() {
        let v = homogeneous();
        let crushed = crush_lossless(&v).expect("should crush");
        // Constants appear once in _defaults, not on every item.
        assert!(crushed.contains("_defaults"));
        assert_eq!(crushed.matches("success").count(), 1);
    }

    #[test]
    fn lossless_roundtrips_exactly() {
        let v = homogeneous();
        let crushed = crush_lossless(&v).unwrap();
        let restored = reconstruct(&crushed).unwrap();
        assert_eq!(restored, v);
    }

    #[test]
    fn output_is_byte_stable_across_calls() {
        let v = homogeneous();
        let run = || crush_lossless(&v).unwrap();
        assert_eq!(run(), run(), "crush output must be deterministic");
    }

    #[test]
    fn never_inflates_compressible_payload() {
        let v = homogeneous();
        let crushed = crush_lossless(&v).unwrap();
        let compact = serde_json::to_string(&v).unwrap();
        assert!(
            crushed.len() < compact.len(),
            "crushed {} should be shorter than {}",
            crushed.len(),
            compact.len()
        );
    }

    #[test]
    fn small_or_heterogeneous_arrays_are_skipped() {
        assert!(crush_lossless(&json!([{"a": 1}, {"a": 2}])).is_none()); // < MIN_ITEMS
        assert!(crush_lossless(&json!([1, 2, 3, 4])).is_none()); // not objects
        assert!(crush_lossless(&json!([])).is_none());
        // No shared identical column -> nothing to factor.
        assert!(
            crush_lossless(&json!([
                {"a": 1}, {"b": 2}, {"c": 3}, {"d": 4}
            ]))
            .is_none()
        );
    }

    #[test]
    fn nested_arrays_crush_and_roundtrip() {
        let v = json!({
            "total": 2,
            "data": [
                {"kind": "node", "ready": true, "name": "a"},
                {"kind": "node", "ready": true, "name": "b"},
                {"kind": "node", "ready": true, "name": "c"}
            ]
        });
        let crushed = crush_lossless(&v).unwrap();
        assert_eq!(reconstruct(&crushed).unwrap(), v);
    }

    #[test]
    fn unicode_and_escapes_survive_roundtrip() {
        let v = json!([
            {"tag": "café", "note": "line\nbreak", "id": 1},
            {"tag": "café", "note": "quote\"x", "id": 2},
            {"tag": "café", "note": "tab\tend", "id": 3}
        ]);
        let crushed = crush_lossless(&v).unwrap();
        assert_eq!(reconstruct(&crushed).unwrap(), v);
    }

    #[test]
    fn dominant_value_factoring_keeps_deviations() {
        let v = json!([
            {"state": "ok", "code": 200},
            {"state": "ok", "code": 200},
            {"state": "ok", "code": 200},
            {"state": "err", "code": 500}
        ]);
        let crushed = crush_lossless(&v).unwrap();
        // The minority row keeps its own state/code; majority omit them.
        assert!(crushed.contains("err"));
        assert_eq!(reconstruct(&crushed).unwrap(), v);
    }

    #[test]
    fn beneficial_helpers_share_one_core_and_threshold() {
        // Redundant array -> both gates return the compact, reconstructible form.
        // Enough rows + constant columns that the compact form clearly halves it.
        let items: Vec<Value> = (0..16)
            .map(|i| {
                json!({"status": "active", "region": "eu-central-1", "tier": "standard", "id": i})
            })
            .collect();
        let v = Value::Array(items);
        let raw = serde_json::to_string(&v).unwrap();
        let by_value = crush_value_if_beneficial(&v, raw.len()).expect("value gate crushes");
        let by_text = crush_text_if_beneficial(&raw).expect("text gate crushes");
        assert_eq!(by_value, by_text, "both gates use one core + threshold");
        assert!(by_text.len() * KEEP_DATA_DIVISOR <= raw.len());
        assert_eq!(reconstruct(&by_text).unwrap(), v);

        // Low-redundancy -> None (caller keeps its own outline/verbatim form).
        let hetero = r#"[{"id":1,"k":"aaa"},{"id":2,"k":"bbb"},{"id":3,"k":"ccc"}]"#;
        assert!(crush_text_if_beneficial(hetero).is_none());

        // Non-JSON and bare scalars -> None.
        assert!(crush_text_if_beneficial("not json").is_none());
        assert!(crush_text_if_beneficial("\"just a string\"").is_none());
        assert!(crush_text_if_beneficial("").is_none());
    }

    #[test]
    fn input_with_marker_key_is_left_alone() {
        let v = json!([
            {"_lc_crush": "x", "id": 1},
            {"_lc_crush": "y", "id": 2},
            {"_lc_crush": "z", "id": 3}
        ]);
        assert!(crush_lossless(&v).is_none());
    }
}
