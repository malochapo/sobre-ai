//! One usage event as an OTLP/JSON log record.
//!
//! The shape follows the OTLP JSON encoding of `LogRecord`: `timeUnixNano` and
//! 64-bit integers as decimal strings, attributes as a list of `{key, value}`
//! with a typed `AnyValue`. Only the fields the ledger uses are written.

use serde_json::{Map, Value as Json, json};

/// An attribute value (the subset of OTLP `AnyValue` the ledger needs).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(u64),
    Double(f64),
    Bool(bool),
}

/// One event: a name, a timestamp and its attributes, in insertion order.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub time_unix_nano: u64,
    pub name: String,
    pub attributes: Vec<(String, Value)>,
}

impl Event {
    #[must_use]
    pub fn now(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        Self {
            time_unix_nano: nanos,
            name: name.to_string(),
            attributes: Vec::new(),
        }
    }

    pub fn attr(&mut self, key: &str, value: Value) {
        self.attributes.push((key.to_string(), value));
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    #[must_use]
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn int(&self, key: &str) -> Option<u64> {
        match self.get(key)? {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn double(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            Value::Double(n) => Some(*n),
            #[allow(clippy::cast_precision_loss)]
            Value::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    #[must_use]
    pub fn bool(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Value::Bool(true)))
    }

    /// Seconds since the epoch, for grouping by day.
    #[must_use]
    pub fn unix_secs(&self) -> u64 {
        self.time_unix_nano / 1_000_000_000
    }

    #[must_use]
    pub fn to_json_line(&self) -> String {
        let attributes: Vec<Json> = self
            .attributes
            .iter()
            .map(|(k, v)| {
                let value = match v {
                    Value::Str(s) => json!({ "stringValue": s }),
                    Value::Int(n) => json!({ "intValue": n.to_string() }),
                    Value::Double(n) => json!({ "doubleValue": n }),
                    Value::Bool(b) => json!({ "boolValue": b }),
                };
                json!({ "key": k, "value": value })
            })
            .collect();
        json!({
            "timeUnixNano": self.time_unix_nano.to_string(),
            "eventName": self.name,
            "attributes": attributes,
        })
        .to_string()
    }

    /// Parses one line; `None` for anything that is not a record this module
    /// wrote (blank lines, a truncated last line after a crash).
    #[must_use]
    pub fn from_json_line(line: &str) -> Option<Self> {
        let v: Json = serde_json::from_str(line.trim()).ok()?;
        let obj = v.as_object()?;
        let time_unix_nano = obj.get("timeUnixNano")?.as_str()?.parse().ok()?;
        let name = obj.get("eventName")?.as_str()?.to_string();
        let attributes = obj
            .get("attributes")?
            .as_array()?
            .iter()
            .filter_map(|a| {
                let key = a.get("key")?.as_str()?.to_string();
                let value = parse_value(a.get("value")?.as_object()?)?;
                Some((key, value))
            })
            .collect();
        Some(Self {
            time_unix_nano,
            name,
            attributes,
        })
    }
}

fn parse_value(v: &Map<String, Json>) -> Option<Value> {
    if let Some(s) = v.get("stringValue") {
        return Some(Value::Str(s.as_str()?.to_string()));
    }
    if let Some(n) = v.get("intValue") {
        // OTLP/JSON writes int64 as a string; accept a bare number too.
        return n
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| n.as_u64())
            .map(Value::Int);
    }
    if let Some(n) = v.get("doubleValue") {
        return n.as_f64().map(Value::Double);
    }
    if let Some(b) = v.get("boolValue") {
        return b.as_bool().map(Value::Bool);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_is_otlp_json_and_round_trips() {
        let mut ev = Event::now("sobre.compression");
        ev.attr("sobre.tool", Value::Str("ctx_read".into()));
        ev.attr("sobre.tokens.original", Value::Int(1200));
        ev.attr("sobre.cost.usd", Value::Double(0.25));
        ev.attr("sobre.upstream.local", Value::Bool(true));
        let line = ev.to_json_line();

        let v: Json = serde_json::from_str(&line).unwrap();
        assert!(v["timeUnixNano"].is_string());
        assert_eq!(v["attributes"][1]["value"]["intValue"], "1200");

        assert_eq!(Event::from_json_line(&line), Some(ev));
    }

    #[test]
    fn truncated_line_is_skipped() {
        assert_eq!(
            Event::from_json_line(r#"{"timeUnixNano":"1","eventN"#),
            None
        );
        assert_eq!(Event::from_json_line(""), None);
    }
}
