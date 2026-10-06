//! Stable identity of the client-cached prompt prefix: a per-conversation key,
//! a hash of the cached prefix, and the cache TTL its `cache_control` markers
//! declare. Every hash ignores the volatile `cache_control` markers themselves,
//! which clients move as the prompt grows. Used by
//! [`crate::proxy::cache_attribution`] to tell a TTL lapse from a prefix change.

use std::hash::{Hash, Hasher};

use serde_json::Value;

/// Anthropic default cache TTL when a `cache_control` marker carries no explicit
/// `ttl` (the API default is "5m").
const DEFAULT_TTL_SECS: u64 = 300;
/// Anthropic extended cache TTL (`"ttl":"1h"`).
const HOUR_TTL_SECS: u64 = 3600;

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Stable per-conversation key: a hash of the first message with every
/// `cache_control` marker stripped first. `messages[0]` is byte-stable across a
/// conversation's turns *except* for its volatile cache breakpoint — clients move
/// or retune the `cache_control` (`ephemeral`/`ttl`) marker as the prompt grows.
/// Hashing the raw message would then change the key mid-conversation → a
/// permanent "first sighting" that never repacks (#499). Stripping the marker
/// keys on stable content only. Distinct conversations still differ (distinct
/// opening turns); a collision can only make the recorded last-touch *more
/// recent*, biasing toward "warm" — never a wrong "cold". `None` when there is no
/// first message.
pub(crate) fn conversation_key(messages: &[Value]) -> Option<u64> {
    let mut first = messages.first()?.clone();
    strip_cache_control(&mut first);
    let bytes = serde_json::to_vec(&first).ok()?;
    Some(hash_bytes(&bytes))
}

/// Content hash of the client-cached prefix `messages[0..cached]` with every
/// volatile `cache_control` marker stripped — the stable identity of the prefix
/// a provider would cache. Turn-to-turn equality means the cacheable prefix did
/// not change; inequality means something (a rewrite, an edited earlier turn)
/// busted it. `None` when `cached == 0` (nothing anchored to compare). Shared so
/// [`crate::proxy::cache_attribution`] keys on the exact same stable bytes.
pub(crate) fn cached_prefix_hash(messages: &[Value], cached: usize) -> Option<u64> {
    let end = cached.min(messages.len());
    if end == 0 {
        return None;
    }
    let mut prefix: Vec<Value> = messages[..end].to_vec();
    for msg in &mut prefix {
        strip_cache_control(msg);
    }
    let bytes = serde_json::to_vec(&prefix).ok()?;
    Some(hash_bytes(&bytes))
}

/// Recursively remove every `cache_control` field so a moving cache breakpoint
/// can't change the conversation key. The marker always nests `type:ephemeral`
/// and any `ttl` inside `cache_control`, so dropping that one field suffices.
fn strip_cache_control(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.remove("cache_control");
            for val in map.values_mut() {
                strip_cache_control(val);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                strip_cache_control(val);
            }
        }
        _ => {}
    }
}

fn parse_ttl_str(s: &str) -> Option<u64> {
    match s.trim() {
        "1h" => Some(HOUR_TTL_SECS),
        "5m" => Some(DEFAULT_TTL_SECS),
        _ => None,
    }
}

/// Largest `cache_control.ttl` declared anywhere inside one message (message-,
/// block-, or nested text-level), in seconds. `None` when no parseable ttl.
fn max_ttl_in_message(msg: &Value) -> Option<u64> {
    let mut best: Option<u64> = None;
    collect_cc_ttl(msg, &mut best);
    best
}

fn collect_cc_ttl(v: &Value, best: &mut Option<u64>) {
    match v {
        Value::Object(map) => {
            if let Some(ttl) = map
                .get("cache_control")
                .and_then(|cc| cc.get("ttl"))
                .and_then(Value::as_str)
                .and_then(parse_ttl_str)
            {
                *best = Some(best.map_or(ttl, |b| b.max(ttl)));
            }
            for val in map.values() {
                collect_cc_ttl(val, best);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                collect_cc_ttl(val, best);
            }
        }
        _ => {}
    }
}

/// Resolve the cache TTL (seconds) for the client-cached prefix `[0..cached)`.
/// Returns the largest TTL any cached message requested, defaulting to the "5m"
/// Anthropic default because a `cache_control` marker is present. `None` only
/// when `cached == 0` (no marker) — in which case there is nothing to repack.
pub(crate) fn resolved_ttl_secs(messages: &[Value], cached: usize) -> Option<u64> {
    if cached == 0 {
        return None;
    }
    let end = cached.min(messages.len());
    let mut ttl = DEFAULT_TTL_SECS;
    for msg in &messages[..end] {
        if let Some(t) = max_ttl_in_message(msg) {
            ttl = ttl.max(t);
        }
    }
    Some(ttl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cached_body(first_text: &str, ttl: Option<&str>) -> Vec<Value> {
        let cc = ttl.map_or_else(
            || json!({"type": "ephemeral"}),
            |t| json!({"type": "ephemeral", "ttl": t}),
        );
        vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": first_text, "cache_control": cc}
            ]}),
            json!({"role": "assistant", "content": "ok"}),
        ]
    }

    #[test]
    fn key_is_stable_across_turns_and_distinct_per_conversation() {
        let mut a1 = cached_body("conversation A opening", None);
        let a2 = {
            let mut m = a1.clone();
            m.push(json!({"role": "user", "content": "a follow-up turn"}));
            m
        };
        let b1 = cached_body("conversation B opening", None);

        let ka = conversation_key(&a1).unwrap();
        let ka2 = conversation_key(&a2).unwrap();
        let kb = conversation_key(&b1).unwrap();
        assert_eq!(ka, ka2, "key must be stable as the conversation grows");
        assert_ne!(ka, kb, "distinct conversations must get distinct keys");

        // Mutating messages[0] changes the key (different conversation head).
        a1[0] = json!({"role": "user", "content": "different head"});
        assert_ne!(conversation_key(&a1).unwrap(), ka);
    }

    #[test]
    fn ttl_resolves_from_marker_else_default_else_none() {
        let hour = cached_body("x", Some("1h"));
        assert_eq!(resolved_ttl_secs(&hour, 1), Some(HOUR_TTL_SECS));

        let five = cached_body("x", Some("5m"));
        assert_eq!(resolved_ttl_secs(&five, 1), Some(DEFAULT_TTL_SECS));

        // Marker present without an explicit ttl → Anthropic "5m" default.
        let bare = cached_body("x", None);
        assert_eq!(resolved_ttl_secs(&bare, 1), Some(DEFAULT_TTL_SECS));

        // No client-cached prefix → nothing to repack.
        assert_eq!(resolved_ttl_secs(&bare, 0), None);
    }

    #[test]
    fn key_ignores_cache_control_marker() {
        // #499 (3): the same opening content with a different — or absent —
        // cache_control marker must map to the SAME conversation key, so a moving
        // cache breakpoint never causes a permanent first-sighting.
        let none = cached_body("marker-invariant conversation", None);
        let hour = cached_body("marker-invariant conversation", Some("1h"));
        let five = cached_body("marker-invariant conversation", Some("5m"));
        let k = conversation_key(&none).unwrap();
        assert_eq!(k, conversation_key(&hour).unwrap());
        assert_eq!(k, conversation_key(&five).unwrap());
        // Different opening content still yields a different key.
        let other = cached_body("a different opening", Some("1h"));
        assert_ne!(k, conversation_key(&other).unwrap());
    }
}
