use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::core::context_radar::RadarEvent;

static LAST_LCTX_CALL_TS: AtomicU64 = AtomicU64::new(0);
static HINT_COOLDOWN: AtomicU32 = AtomicU32::new(0);
static SESSION_ID: Mutex<Option<String>> = Mutex::new(None);
static SERVER_START_TS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

const COOLDOWN_CALLS: u32 = 5;

/// Whether bypass hints are enabled (independent of `minimal_overhead`).
pub fn is_enabled() -> bool {
    effective_mode() != "off"
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

const NATIVE_READ_TOOLS: &[&str] = &[
    "Read",
    "read",
    "read_file",
    "ReadFile",
    "Grep",
    "grep",
    "search",
    "ripgrep",
];

pub fn record_lctx_call() {
    LAST_LCTX_CALL_TS.store(now_millis(), Ordering::Relaxed);
}

pub fn set_session_id(id: &str) {
    if let Ok(mut guard) = SESSION_ID.lock() {
        let changed = guard.as_deref() != Some(id);
        *guard = Some(id.to_string());
        if changed {
            LAST_LCTX_CALL_TS.store(0, Ordering::Relaxed);
            HINT_COOLDOWN.store(0, Ordering::Relaxed);
        }
    }
}

pub fn check(data_dir: &Path) -> Option<String> {
    let mode = effective_mode();
    if mode == "off" {
        return None;
    }

    let shadow = true;
    let aggressive = mode == "aggressive" || shadow;

    if !aggressive {
        let counter = HINT_COOLDOWN.fetch_add(1, Ordering::Relaxed);
        if !counter.is_multiple_of(COOLDOWN_CALLS) {
            return None;
        }
    }

    // Gate 2 fix: when no ctx_* call has happened yet (cold start), fall back
    // to the server start timestamp so we still detect native tool drift.
    let last_ts = LAST_LCTX_CALL_TS.load(Ordering::Relaxed);
    let baseline = if last_ts > 0 {
        last_ts
    } else {
        *SERVER_START_TS.get_or_init(now_millis)
    };

    let session_id = SESSION_ID.lock().ok().and_then(|g| g.clone());

    // Gate 3 fix: if session-filtered count is 0 (e.g. Cursor UUID vs lean-ctx
    // session id mismatch), retry unfiltered to avoid false negatives.
    let mut native_count = count_native_since(data_dir, baseline, session_id.as_deref());
    if native_count == 0 && session_id.is_some() {
        native_count = count_native_since(data_dir, baseline, None);
    }
    if native_count == 0 {
        return None;
    }

    if shadow {
        Some(format!(
            "\n[SHADOW MODE: This native Read/Grep call was intercepted ({native_count}x). \
             Use ctx_read/ctx_search directly — faster, cached, saves ~87% tokens.]"
        ))
    } else {
        Some(format!(
            "\n[HINT: You used native Read/Grep {native_count}x since your last ctx_read call. \
             Use ctx_read/ctx_search instead — cached, re-reads ~13 tok, saves ~87% tokens.]"
        ))
    }
}

fn count_native_since(data_dir: &Path, since_ts: u64, session_id: Option<&str>) -> usize {
    let radar_path = radar_jsonl_path(data_dir);
    if !radar_path.exists() {
        return 0;
    }

    let Ok(content) = std::fs::read_to_string(&radar_path) else {
        return 0;
    };

    let mut count = 0;
    for line in content.lines().rev() {
        if line.is_empty() {
            continue;
        }
        let event: RadarEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let event_ts_ms = event.ts * 1000;
        if event_ts_ms < since_ts {
            break;
        }

        // Only count events from the same session (avoids subagent and
        // parallel-tab false positives). Events without a conversation_id
        // are excluded when session filtering is active — they come from
        // IDE-internal hooks or background processes, not agent tool calls.
        if let Some(sid) = session_id {
            match event.conversation_id.as_deref() {
                Some(event_sid) if event_sid == sid => {}
                _ => continue,
            }
        }

        if event.event_type == "native_tool" {
            if !is_read_grep_tool(event.tool_name.as_ref()) {
                continue;
            }
            if let Some(ref name) = event.tool_name
                && (name.starts_with("ctx_") || name.starts_with("mcp__lean-ctx__"))
            {
                continue;
            }
            count += 1;
        }
        if event.event_type == "file_read" && is_read_grep_tool(event.tool_name.as_ref()) {
            count += 1;
        }
    }
    count
}

fn is_read_grep_tool(tool_name: Option<&String>) -> bool {
    tool_name.is_some_and(|name| NATIVE_READ_TOOLS.iter().any(|t| name == *t))
}

fn effective_mode() -> String {
    None::<String>.as_deref().unwrap_or("on").to_lowercase()
}

/// Returns `true` if no lean-ctx MCP calls have been seen in the last 5 minutes.
/// Used by the redirect-suffix logic to append a nudge to `.lctx` temp files
/// when the model appears to be drifting away from ctx_* tools.
pub fn model_is_drifting(data_dir: &Path) -> bool {
    let radar_path = radar_jsonl_path(data_dir);
    if !radar_path.exists() {
        return true;
    }
    let Ok(content) = std::fs::read_to_string(&radar_path) else {
        return true;
    };
    let five_min_ago = now_millis().saturating_sub(5 * 60 * 1000);
    for line in content.lines().rev() {
        if line.is_empty() {
            continue;
        }
        let event: RadarEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let event_ts_ms = event.ts * 1000;
        if event_ts_ms < five_min_ago {
            break;
        }
        if event.event_type == "mcp_call" {
            let is_lctx = event
                .tool_name
                .as_deref()
                .is_some_and(|t| t.starts_with("ctx_"))
                || event
                    .detail
                    .as_deref()
                    .is_some_and(|d| d.contains("lean-ctx"));
            if is_lctx {
                return false;
            }
        }
    }
    true
}

pub const REDIRECT_SUFFIX: &str =
    "\n--- lean-ctx: ctx_compose bundles search+read+symbols in one call ---";

fn radar_jsonl_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("context_radar.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn no_hint_when_no_native_events() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_radar.jsonl");
        std::fs::write(&path, "").unwrap();
        LAST_LCTX_CALL_TS.store(1_000_000, Ordering::Relaxed);
        assert_eq!(count_native_since(dir.path(), 1_000_000, None), 0);
    }

    #[test]
    fn only_counts_read_grep_not_edit_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_radar.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"ts":1100,"event_type":"native_tool","tokens":200,"tool_name":"Read"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"ts":1200,"event_type":"native_tool","tokens":150,"tool_name":"Grep"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"ts":1300,"event_type":"native_tool","tokens":100,"tool_name":"Edit"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"ts":1400,"event_type":"native_tool","tokens":100,"tool_name":"Write"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"ts":1500,"event_type":"native_tool","tokens":100,"tool_name":"Shell"}}"#
        )
        .unwrap();
        drop(f);

        // Only Read + Grep count (2), not Edit/Write/Shell
        assert_eq!(count_native_since(dir.path(), 1_000_000, None), 2);
    }

    #[test]
    fn file_read_without_tool_name_not_counted() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_radar.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"ts":1100,"event_type":"file_read","tokens":100}}"#).unwrap();
        writeln!(
            f,
            r#"{{"ts":1200,"event_type":"file_read","tokens":100,"tool_name":"Read"}}"#
        )
        .unwrap();
        // file_read with non-Read tool_name should NOT count
        writeln!(
            f,
            r#"{{"ts":1300,"event_type":"file_read","tokens":100,"tool_name":"SomePlugin"}}"#
        )
        .unwrap();
        drop(f);

        assert_eq!(count_native_since(dir.path(), 1_000_000, None), 1);
    }

    #[test]
    fn session_filter_excludes_events_without_conversation_id() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_radar.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        // Event with matching session
        writeln!(f, r#"{{"ts":1100,"event_type":"native_tool","tokens":200,"tool_name":"Read","conversation_id":"sess-1"}}"#).unwrap();
        // Event WITHOUT conversation_id (IDE background, hooks, etc.)
        writeln!(
            f,
            r#"{{"ts":1200,"event_type":"native_tool","tokens":150,"tool_name":"Read"}}"#
        )
        .unwrap();
        drop(f);

        // With session filter: only the matching event counts, not the one without ID
        assert_eq!(count_native_since(dir.path(), 1_000_000, Some("sess-1")), 1);
        // Without session filter: both count
        assert_eq!(count_native_since(dir.path(), 1_000_000, None), 2);
    }

    #[test]
    fn session_filter_excludes_other_sessions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_radar.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"ts":1100,"event_type":"native_tool","tokens":200,"tool_name":"Read","conversation_id":"session-A"}}"#).unwrap();
        writeln!(f, r#"{{"ts":1200,"event_type":"native_tool","tokens":150,"tool_name":"Grep","conversation_id":"session-B"}}"#).unwrap();
        writeln!(f, r#"{{"ts":1300,"event_type":"native_tool","tokens":100,"tool_name":"Read","conversation_id":"session-A"}}"#).unwrap();
        drop(f);

        // Filter for session-A: only 2 events
        assert_eq!(
            count_native_since(dir.path(), 1_000_000, Some("session-A")),
            2
        );
        // Filter for session-B: only 1 event
        assert_eq!(
            count_native_since(dir.path(), 1_000_000, Some("session-B")),
            1
        );
    }

    #[test]
    fn ignores_ctx_tools_in_native_events() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_radar.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"ts":1100,"event_type":"native_tool","tokens":200,"tool_name":"ctx_read"}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"ts":1200,"event_type":"native_tool","tokens":150,"tool_name":"mcp__lean-ctx__ctx_search"}}"#).unwrap();
        writeln!(
            f,
            r#"{{"ts":1300,"event_type":"native_tool","tokens":100,"tool_name":"Read"}}"#
        )
        .unwrap();
        drop(f);

        assert_eq!(count_native_since(dir.path(), 1_000_000, None), 1);
    }

    #[test]
    fn millis_timestamp_precision() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_radar.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"ts":5,"event_type":"native_tool","tokens":100,"tool_name":"Read"}}"#
        )
        .unwrap();
        drop(f);

        assert_eq!(count_native_since(dir.path(), 5500, None), 0);
        assert_eq!(count_native_since(dir.path(), 4999, None), 1);
    }

    // ── Gate 2: cold-start fallback ─────────────────────────────

    // ── Gate 3: session-ID fallback ──────────────────────────────

    // ── model_is_drifting ────────────────────────────────────────

    #[test]
    fn drifting_when_no_radar_file() {
        let dir = TempDir::new().unwrap();
        assert!(model_is_drifting(dir.path()));
    }

    #[test]
    fn drifting_when_empty_radar() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("context_radar.jsonl"), "").unwrap();
        assert!(model_is_drifting(dir.path()));
    }

    #[test]
    fn not_drifting_when_recent_lctx_call() {
        let dir = TempDir::new().unwrap();
        let now_secs = now_millis() / 1000;
        let mut f = std::fs::File::create(dir.path().join("context_radar.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"ts":{now_secs},"event_type":"mcp_call","tokens":100,"tool_name":"ctx_read"}}"#,
        )
        .unwrap();
        drop(f);
        assert!(!model_is_drifting(dir.path()));
    }

    #[test]
    fn drifting_when_only_old_lctx_calls() {
        let dir = TempDir::new().unwrap();
        let old_secs = (now_millis() / 1000).saturating_sub(600);
        let mut f = std::fs::File::create(dir.path().join("context_radar.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"ts":{old_secs},"event_type":"mcp_call","tokens":100,"tool_name":"ctx_read"}}"#,
        )
        .unwrap();
        drop(f);
        assert!(model_is_drifting(dir.path()));
    }

    // ── is_enabled ───────────────────────────────────────────────
}
