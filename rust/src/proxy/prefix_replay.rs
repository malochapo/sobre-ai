//! Append-only delta detection and forwarded-byte replay for provider prefix
//! cache stability.
//!
//! When the proxy compresses and forwards a request, the serialised prefix
//! bytes become the provider's cache key. Re-serialising an identical `Value`
//! can produce subtly different bytes (JSON key order, float precision, Unicode
//! escaping), causing a cache miss even though the content has not changed.
//!
//! This module caches the **exact forwarded bytes** and replays them verbatim
//! on subsequent turns when the new request is an append-only extension.
//! Only the delta (new messages) is freshly serialised.
//!
//! Active only in `ProxyMode::Cache`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

const MAX_TRACKED: usize = 2048;

#[derive(Clone)]
struct ConversationPrefix {
    forwarded_bytes: Vec<u8>,
    original_hashes: Vec<u64>,
    count: usize,
}

pub(crate) struct AppendDelta {
    pub prefix_bytes: Vec<u8>,
    pub delta_start: usize,
}

fn store() -> &'static Mutex<HashMap<u64, ConversationPrefix>> {
    static STORE: OnceLock<Mutex<HashMap<u64, ConversationPrefix>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn message_hash(msg: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let canonical = serde_json::to_string(msg).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    hasher.finish()
}

/// Conversation identity from system prompt + first user message.
pub(crate) fn conversation_id(system: Option<&Value>, messages: &[Value]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(sys) = system {
        serde_json::to_string(sys)
            .unwrap_or_default()
            .hash(&mut hasher);
    }
    if let Some(first) = messages.first() {
        serde_json::to_string(first)
            .unwrap_or_default()
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// Detect whether current messages are an append-only extension of the previous
/// turn. Returns cached prefix bytes and delta start index if so.
pub(crate) fn detect_append_only(conv_id: u64, messages: &[Value]) -> Option<AppendDelta> {
    let guard = store().lock().ok()?;
    let prev = guard.get(&conv_id)?;

    if messages.len() <= prev.count {
        return None;
    }

    let current_hashes: Vec<u64> = messages.iter().map(message_hash).collect();
    for (i, prev_hash) in prev.original_hashes.iter().enumerate() {
        if current_hashes.get(i) != Some(prev_hash) {
            return None;
        }
    }

    Some(AppendDelta {
        prefix_bytes: prev.forwarded_bytes.clone(),
        delta_start: prev.count,
    })
}

/// Record the forwarded prefix bytes after a successful upstream send.
pub(crate) fn record_forwarded(
    conv_id: u64,
    forwarded: Vec<u8>,
    originals: &[Value],
    msg_count: usize,
) {
    let hashes: Vec<u64> = originals.iter().take(msg_count).map(message_hash).collect();
    let entry = ConversationPrefix {
        forwarded_bytes: forwarded,
        original_hashes: hashes,
        count: msg_count,
    };

    if let Ok(mut guard) = store().lock() {
        if guard.len() >= MAX_TRACKED
            && !guard.contains_key(&conv_id)
            && let Some(&oldest) = guard.keys().next()
        {
            guard.remove(&oldest);
        }
        guard.insert(conv_id, entry);
    }
}

/// Overlay cached prefix bytes with fresh delta bytes, producing a valid JSON
/// array where the prefix portion is byte-identical to the previous forward.
pub(crate) fn overlay_prefix(prefix_bytes: &[u8], delta_messages: &[Value]) -> Option<Vec<u8>> {
    if delta_messages.is_empty() {
        return Some(prefix_bytes.to_vec());
    }

    let prefix_str = std::str::from_utf8(prefix_bytes).ok()?;
    let trimmed = prefix_str.trim_end();
    if !trimmed.ends_with(']') {
        return None;
    }
    let without_bracket = &trimmed[..trimmed.len() - 1];

    let mut result = without_bracket.as_bytes().to_vec();
    for msg in delta_messages {
        result.extend_from_slice(b",");
        let serialised = serde_json::to_string(msg).ok()?;
        result.extend_from_slice(serialised.as_bytes());
    }
    result.push(b']');
    Some(result)
}

#[cfg(test)]
pub(crate) fn clear() {
    if let Ok(mut guard) = store().lock() {
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_messages() -> Vec<Value> {
        vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi there"}),
        ]
    }

    #[test]
    fn append_only_detection_works() {
        clear();
        let msgs = sample_messages();
        let conv = conversation_id(None, &msgs);
        let forwarded = serde_json::to_vec(&msgs).unwrap();

        record_forwarded(conv, forwarded.clone(), &msgs, msgs.len());

        let mut extended = msgs.clone();
        extended.push(json!({"role": "user", "content": "what's up?"}));

        let delta = detect_append_only(conv, &extended).expect("should detect append-only");
        assert_eq!(delta.delta_start, 2);
        assert_eq!(delta.prefix_bytes, forwarded);
    }

    #[test]
    fn detection_fails_on_modified_prefix() {
        clear();
        let msgs = sample_messages();
        let conv = conversation_id(None, &msgs);
        let forwarded = serde_json::to_vec(&msgs).unwrap();
        record_forwarded(conv, forwarded, &msgs, msgs.len());

        let mut modified = msgs;
        modified[0] = json!({"role": "user", "content": "different"});
        modified.push(json!({"role": "user", "content": "extra"}));
        assert!(detect_append_only(conv, &modified).is_none());
    }

    #[test]
    fn prefix_replay_is_byte_identical_across_turns() {
        clear();
        let msgs = sample_messages();
        let conv = conversation_id(None, &msgs);
        let forwarded = serde_json::to_vec(&msgs).unwrap();
        record_forwarded(conv, forwarded.clone(), &msgs, msgs.len());

        let mut turn2 = msgs.clone();
        turn2.push(json!({"role": "user", "content": "next"}));

        let delta = detect_append_only(conv, &turn2).unwrap();
        let result = overlay_prefix(&delta.prefix_bytes, &turn2[delta.delta_start..]).unwrap();
        let result_str = String::from_utf8(result).unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result_str).unwrap();
        assert_eq!(parsed.len(), 3);

        let prefix_portion = &result_str[..forwarded.len() - 1];
        let original_prefix = std::str::from_utf8(&forwarded[..forwarded.len() - 1]).unwrap();
        assert_eq!(
            prefix_portion, original_prefix,
            "prefix bytes must be identical"
        );
    }

    #[test]
    fn overlay_with_empty_delta_returns_prefix() {
        let msgs = sample_messages();
        let bytes = serde_json::to_vec(&msgs).unwrap();
        let result = overlay_prefix(&bytes, &[]).unwrap();
        assert_eq!(result, bytes);
    }

    #[test]
    fn conversation_id_is_deterministic() {
        let sys = json!("You are helpful");
        let msgs = sample_messages();
        assert_eq!(
            conversation_id(Some(&sys), &msgs),
            conversation_id(Some(&sys), &msgs)
        );
    }

    #[test]
    fn max_tracked_evicts_oldest() {
        clear();
        for i in 0..MAX_TRACKED + 10 {
            let msgs = vec![json!({"role": "user", "content": format!("msg {i}")})];
            let conv = conversation_id(None, &msgs);
            record_forwarded(conv, serde_json::to_vec(&msgs).unwrap(), &msgs, 1);
        }
        assert!(store().lock().unwrap().len() <= MAX_TRACKED);
    }
}
