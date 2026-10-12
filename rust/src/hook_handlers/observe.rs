//! Observe hook handler: reads every host hook event and keeps what sobre
//! uses from it: the active transcript and session id, and a marker when the
//! host compacts the conversation.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Unified observe handler for all IDE hook events.
/// Reads JSON from stdin, records the transcript it names, handles
/// compaction, and exits immediately.
pub fn handle_observe() {
    let Some(input) = read_stdin_with_timeout(HOOK_STDIN_TIMEOUT) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&input) else {
        return;
    };
    record_session_context(&v);
    if !is_compaction(&v) {
        return;
    }
    // Compaction evicts the conversation the read-dedup stubs would point into
    //: purge the session's re-read records so every file delivers
    // full content again, mirroring the MCP-side compaction sync.
    if let Some(session_id) = v.get("session_id").and_then(|s| s.as_str()) {
        super::read_dedup::purge_session(session_id);
    }
    if let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        write_compaction_marker(&data_dir, ts);
    }
}

fn non_empty_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|x| !x.is_empty())
}

fn record_session_context(v: &serde_json::Value) {
    // Claude Code / Codex carry a per-session `session_id` but no
    // `conversation_id`. Persisting it lets the read-cache scope a session's
    // deliveries the same way Cursor's `conversation_id` does, so a new session
    // never inherits a prior one's `[unchanged]` stubs.
    if let Some(tp) = non_empty_str(v, "transcript_path") {
        persist_transcript_path(
            tp,
            non_empty_str(v, "conversation_id"),
            non_empty_str(v, "session_id"),
        );
    }
}

/// Claude Code emits `hook_event_name: "PreCompact"` (code.claude.com/docs/
/// en/hooks); the generic `event`/`compaction` shapes cover other hosts.
/// Every Claude hook payload also carries `session_id`, so this must not be
/// confused with a plain session event.
fn is_compaction(v: &serde_json::Value) -> bool {
    v.get("compaction").is_some()
        || v.get("messages_count").is_some()
        || v.get("hook_event_name")
            .and_then(|e| e.as_str())
            .is_some_and(|e| e == "PreCompact")
        || v.get("event")
            .and_then(|e| e.as_str())
            .is_some_and(|e| e == "compaction" || e == "compact")
}

fn persist_transcript_path(path: &str, conversation_id: Option<&str>, session_id: Option<&str>) {
    let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() else {
        return;
    };
    let meta_path = data_dir.join("active_transcript.json");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let payload = serde_json::json!({
        "transcript_path": path,
        "conversation_id": conversation_id,
        "session_id": session_id,
        "updated_at": ts,
    });
    if let Ok(json) = serde_json::to_string_pretty(&payload) {
        let tmp = meta_path.with_extension("tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &meta_path);
        }
    }
}

pub fn load_active_transcript() -> Option<(String, Option<String>)> {
    let data_dir = crate::core::data_dir::sobre_data_dir().ok()?;
    let path = data_dir.join("active_transcript.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let tp = v.get("transcript_path")?.as_str()?.to_string();
    // Prefer Cursor's `conversation_id`; fall back to the host `session_id`
    // (Claude Code / Codex) so the read-cache still has a per-session
    // identity to scope stubs against.
    let conv = v
        .get("conversation_id")
        .and_then(|c| c.as_str())
        .or_else(|| v.get("session_id").and_then(|s| s.as_str()))
        .map(String::from);
    let updated = v.get("updated_at")?.as_u64()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(updated) > 7200 {
        return None;
    }
    Some((tp, conv))
}

/// Atomically write `last_compaction.json` via temp-file + rename.
/// The marker is a few bytes; no other event can displace it.
fn write_compaction_marker(data_dir: &std::path::Path, ts: u64) {
    use std::io::Write;
    let marker_path = data_dir.join("last_compaction.json");
    let tmp_path = data_dir.join("last_compaction.json.tmp");
    let payload = format!(r#"{{"ts":{ts}}}"#);
    let Ok(mut f) = std::fs::File::create(&tmp_path) else {
        return;
    };
    if f.write_all(payload.as_bytes()).is_err() || f.sync_all().is_err() {
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    drop(f);
    let _ = std::fs::rename(&tmp_path, &marker_path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real Claude Code PreCompact payload (code.claude.com/docs/en/hooks):
    /// carries `session_id` like every Claude hook.
    #[test]
    fn claude_precompact_is_compaction() {
        let v = serde_json::json!({
            "session_id": "abc123",
            "transcript_path": "/Users/u/.claude/projects/x/abc123.jsonl",
            "cwd": "/Users/u/project",
            "hook_event_name": "PreCompact",
            "trigger": "auto",
            "custom_instructions": ""
        });
        assert!(is_compaction(&v));
    }

    #[test]
    fn plain_session_and_tool_events_are_not_compaction() {
        for v in [
            serde_json::json!({"session_id": "abc123", "hook_event_name": "SessionStart"}),
            serde_json::json!({"tool_name": "Read", "tool_input": {"path": "src/main.rs"}}),
            serde_json::json!({"toolName": "bash", "toolResult": {"textResultForLlm": "ok"}}),
        ] {
            assert!(!is_compaction(&v), "{v}");
        }
    }

    /// A Claude/Codex payload has no `conversation_id`, so the
    /// persisted `session_id` must surface as the scope id — otherwise the read
    /// cache sees `None` and a new session inherits the prior one's stubs.
    #[test]
    fn session_id_is_used_as_scope_id_when_no_conversation_id() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        persist_transcript_path("/tmp/x/abc.jsonl", None, Some("sess-abc"));
        let (_, conv) = load_active_transcript().expect("marker just written");
        assert_eq!(conv.as_deref(), Some("sess-abc"));
    }

    /// Cursor's `conversation_id` still wins when both are present.
    #[test]
    fn conversation_id_wins_over_session_id() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        persist_transcript_path("/tmp/x/abc.jsonl", Some("conv-1"), Some("sess-abc"));
        let (_, conv) = load_active_transcript().expect("marker just written");
        assert_eq!(conv.as_deref(), Some("conv-1"));
    }
}
