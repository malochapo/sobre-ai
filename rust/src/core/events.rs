use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const JSONL_MAX_LINES: usize = 10_000;
const EVENT_ID_BLOCK_SIZE: u64 = 1_024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeanCtxEvent {
    pub id: u64,
    pub timestamp: String,
    pub kind: EventKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventKind {
    ToolCall {
        tool: String,
        tokens_original: u64,
        tokens_saved: u64,
        mode: Option<String>,
        duration_ms: u64,
        path: Option<String>,
    },
}

/// Appends one event to `events.jsonl` (read by `lean-ctx doctor`).
pub fn emit(kind: EventKind) -> u64 {
    let id = next_event_id();
    let event = LeanCtxEvent {
        id,
        timestamp: chrono::Local::now()
            .format("%Y-%m-%dT%H:%M:%S%.3f")
            .to_string(),
        kind,
    };
    append_jsonl(&event);
    id
}

fn jsonl_path() -> Option<std::path::PathBuf> {
    crate::core::paths::state_dir()
        .ok()
        .map(|d| d.join("events.jsonl"))
}

fn event_sequence_path() -> Option<std::path::PathBuf> {
    crate::core::paths::state_dir()
        .ok()
        .map(|d| d.join("events.seq"))
}

fn max_event_id_in_journal(path: &std::path::Path) -> u64 {
    let Ok(content) = std::fs::read_to_string(path) else {
        return 0;
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<LeanCtxEvent>(line).ok())
        .map(|event| event.id)
        .max()
        .unwrap_or(0)
}

fn parse_sequence_record(record: &str) -> Option<u64> {
    let (value, checksum) = record.trim().split_once(':')?;
    let value = value.parse::<u64>().ok()?;
    let checksum = checksum.parse::<u64>().ok()?;
    (checksum == !value).then_some(value)
}

fn read_persisted_sequence(path: &std::path::Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().rev().find_map(parse_sequence_record)
}

fn sequence_record(value: u64) -> String {
    format!("{value}:{}", !value)
}

fn reserve_event_id_block_at(
    sequence_path: &std::path::Path,
    journal_path: &std::path::Path,
    block_size: u64,
) -> std::io::Result<(u64, u64)> {
    use fs2::FileExt;

    if block_size == 0 {
        return Err(std::io::Error::other("event id block must not be empty"));
    }
    if let Some(parent) = sequence_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Lock a stable companion inode. The append-only sequence journal can be
    // repaired without allowing another process to bypass the lock.
    let lock_path = sequence_path.with_extension("seq.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;

    let result = (|| {
        let persisted = read_persisted_sequence(sequence_path);
        let baseline = persisted.unwrap_or_else(|| {
            max_event_id_in_journal(journal_path).max(max_event_id_in_journal(
                &journal_path.with_extension("jsonl.old"),
            ))
        });
        let first = baseline
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("event id space exhausted"))?;
        let last = baseline
            .checked_add(block_size)
            .ok_or_else(|| std::io::Error::other("event id space exhausted"))?;

        // A prior writer's record (or corruption) may leave the file without
        // a trailing newline; appending directly would fuse onto that line
        // and make both records unparsable. Start a fresh line first.
        let needs_leading_newline = std::fs::read(sequence_path)
            .ok()
            .is_some_and(|bytes| !bytes.is_empty() && bytes.last() != Some(&b'\n'));

        let mut sequence = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(sequence_path)?;
        let mut record = Vec::new();
        if needs_leading_newline {
            record.push(b'\n');
        }
        record.extend_from_slice(sequence_record(last).as_bytes());
        record.push(b'\n');
        sequence.write_all(&record)?;
        sequence.sync_data()?;
        Ok((first, last))
    })();

    let _ = FileExt::unlock(&lock);
    result
}

#[cfg(test)]
fn next_event_id_at(
    sequence_path: &std::path::Path,
    journal_path: &std::path::Path,
) -> std::io::Result<u64> {
    reserve_event_id_block_at(sequence_path, journal_path, 1).map(|(first, _)| first)
}

fn next_event_id() -> u64 {
    #[derive(Default)]
    struct LocalBlock {
        next: u64,
        last: u64,
    }

    static BLOCK: OnceLock<Mutex<LocalBlock>> = OnceLock::new();
    static FALLBACK: AtomicU64 = AtomicU64::new(0);
    if !is_test_environment()
        && let Some((sequence, journal)) = event_sequence_path().zip(jsonl_path())
    {
        let mut block = BLOCK
            .get_or_init(|| Mutex::new(LocalBlock::default()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if block.next <= block.last && block.next != 0 {
            let id = block.next;
            block.next = block.next.saturating_add(1);
            return id;
        }
        if let Ok((first, last)) =
            reserve_event_id_block_at(&sequence, &journal, EVENT_ID_BLOCK_SIZE)
        {
            block.next = first.saturating_add(1);
            block.last = last;
            return first;
        }
    }

    let base = u64::try_from(chrono::Utc::now().timestamp_millis())
        .unwrap_or_default()
        .saturating_mul(1_000);
    base.saturating_add(FALLBACK.fetch_add(1, Ordering::Relaxed))
}

fn is_test_environment() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        if cfg!(test) {
            return true;
        }
        if std::env::var_os("__LEAN_CTX_SKIP_EVENTS").is_some() {
            return true;
        }
        std::env::current_exe().is_ok_and(|p| {
            let s = p.to_string_lossy();
            s.contains("/deps/") || s.contains("\\deps\\")
        })
    })
}

fn append_jsonl(event: &LeanCtxEvent) {
    if is_test_environment() {
        return;
    }
    let Some(path) = jsonl_path() else { return };
    let _ = append_jsonl_at(&path, event);
}

fn append_jsonl_at(path: &std::path::Path, event: &LeanCtxEvent) -> std::io::Result<()> {
    use fs2::FileExt;
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Lock a stable companion inode rather than events.jsonl itself: rotation
    // renames the data file, so locking that inode would let another process
    // open the replacement and bypass the lock.
    let lock_path = path.with_extension("jsonl.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;

    let result = (|| {
        if let Ok(content) = std::fs::read_to_string(path)
            && content.lines().count() >= JSONL_MAX_LINES
        {
            let old = path.with_extension("jsonl.old");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(path, old)?;
        }

        let json = serde_json::to_string(event).map_err(std::io::Error::other)?;
        let mut line = json.into_bytes();
        line.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        // One logical write while holding the interprocess lock prevents JSON
        // objects and their trailing newlines from interleaving.
        file.write_all(&line)
    })();

    let _ = FileExt::unlock(&lock);
    result
}

pub fn emit_tool_call(
    tool: &str,
    tokens_original: u64,
    tokens_saved: u64,
    mode: Option<String>,
    duration_ms: u64,
    path: Option<String>,
) {
    emit(EventKind::ToolCall {
        tool: tool.to_string(),
        tokens_original,
        tokens_saved,
        mode,
        duration_ms,
        path,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_returns_positive_id() {
        let id = emit(EventKind::ToolCall {
            tool: "ctx_read".to_string(),
            tokens_original: 1000,
            tokens_saved: 800,
            mode: Some("map".to_string()),
            duration_ms: 5,
            path: Some("src/main.rs".to_string()),
        });
        assert!(id > 0);
    }

    #[test]
    fn events_jsonl_writer_child() {
        let Some(path) = std::env::var_os("__LEAN_CTX_EVENTS_TEST_PATH") else {
            return;
        };
        let writer = std::env::var("__LEAN_CTX_EVENTS_TEST_WRITER")
            .expect("writer id")
            .parse::<u64>()
            .expect("numeric writer id");
        for sequence in 0..100 {
            let event = LeanCtxEvent {
                id: writer * 100 + sequence,
                timestamp: "2026-07-15T12:00:00.000".to_string(),
                kind: EventKind::ToolCall {
                    tool: "ctx_read".to_string(),
                    tokens_original: sequence,
                    tokens_saved: 0,
                    mode: None,
                    duration_ms: 0,
                    path: Some(format!("writer-{writer}.rs")),
                },
            };
            append_jsonl_at(std::path::Path::new(&path), &event).expect("append event");
        }
    }

    fn test_event(id: u64) -> LeanCtxEvent {
        LeanCtxEvent {
            id,
            timestamp: "2026-07-15T12:00:00.000".to_string(),
            kind: EventKind::ToolCall {
                tool: "ctx_read".to_string(),
                tokens_original: 1,
                tokens_saved: 0,
                mode: None,
                duration_ms: 0,
                path: Some("event-id-test.rs".to_string()),
            },
        }
    }

    #[test]
    fn concurrent_processes_append_complete_json_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("events.jsonl");
        let executable = std::env::current_exe().expect("test executable");

        let mut children = Vec::new();
        for writer in 0..4 {
            children.push(
                std::process::Command::new(&executable)
                    .args([
                        "--exact",
                        "core::events::tests::events_jsonl_writer_child",
                        "--nocapture",
                    ])
                    .env("__LEAN_CTX_EVENTS_TEST_PATH", &path)
                    .env("__LEAN_CTX_EVENTS_TEST_WRITER", writer.to_string())
                    .spawn()
                    .expect("spawn writer"),
            );
        }
        for mut child in children {
            assert!(child.wait().expect("wait for writer").success());
        }

        let content = std::fs::read_to_string(&path).expect("read events");
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 400, "every append must produce one line");
        for line in lines {
            serde_json::from_str::<LeanCtxEvent>(line)
                .unwrap_or_else(|error| panic!("invalid JSONL line: {error}: {line}"));
        }
    }

    #[test]
    fn concurrent_processes_serialize_rotation_and_append() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("events.jsonl");
        let seed = serde_json::to_string(&LeanCtxEvent {
            id: 1,
            timestamp: "2026-07-15T12:00:00.000".to_string(),
            kind: EventKind::ToolCall {
                tool: "ctx_read".to_string(),
                tokens_original: 1,
                tokens_saved: 0,
                mode: None,
                duration_ms: 0,
                path: Some("seed.rs".to_string()),
            },
        })
        .expect("serialize seed");
        std::fs::write(&path, format!("{seed}\n").repeat(JSONL_MAX_LINES))
            .expect("seed rotation threshold");

        let executable = std::env::current_exe().expect("test executable");
        let mut children = Vec::new();
        for writer in 0..4 {
            children.push(
                std::process::Command::new(&executable)
                    .args(["--exact", "core::events::tests::events_jsonl_writer_child"])
                    .env("__LEAN_CTX_EVENTS_TEST_PATH", &path)
                    .env("__LEAN_CTX_EVENTS_TEST_WRITER", writer.to_string())
                    .spawn()
                    .expect("spawn writer"),
            );
        }
        for mut child in children {
            assert!(child.wait().expect("wait for writer").success());
        }

        let old =
            std::fs::read_to_string(path.with_extension("jsonl.old")).expect("rotated journal");
        assert_eq!(old.lines().count(), JSONL_MAX_LINES);
        assert!(
            old.lines()
                .all(|line| serde_json::from_str::<LeanCtxEvent>(line).is_ok()),
            "rotated journal must contain complete JSON lines"
        );

        let current = std::fs::read_to_string(&path).expect("current journal");
        assert_eq!(current.lines().count(), 400);
        assert!(
            current
                .lines()
                .all(|line| serde_json::from_str::<LeanCtxEvent>(line).is_ok()),
            "replacement journal must contain complete JSON lines"
        );
    }

    #[test]
    fn persistent_event_id_writer_child() {
        let Some(dir) = std::env::var_os("__LEAN_CTX_EVENT_ID_TEST_DIR") else {
            return;
        };
        let writer = std::env::var("__LEAN_CTX_EVENT_ID_TEST_WRITER").expect("writer id");
        let dir = std::path::PathBuf::from(dir);
        let sequence = dir.join("events.seq");
        let journal = dir.join("events.jsonl");
        let (first, last) =
            reserve_event_id_block_at(&sequence, &journal, 100).expect("reserve event id block");
        let ids: Vec<String> = (first..=last).map(|id| id.to_string()).collect();
        std::fs::write(dir.join(format!("writer-{writer}.ids")), ids.join("\n"))
            .expect("write allocated ids");
    }

    #[test]
    fn persistent_event_id_bootstraps_above_existing_journals() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sequence = dir.path().join("events.seq");
        let journal = dir.path().join("events.jsonl");
        let old = journal.with_extension("jsonl.old");
        std::fs::write(
            &journal,
            format!("{}\n", serde_json::to_string(&test_event(41)).unwrap()),
        )
        .unwrap();
        std::fs::write(
            &old,
            format!("{}\n", serde_json::to_string(&test_event(73)).unwrap()),
        )
        .unwrap();

        assert_eq!(next_event_id_at(&sequence, &journal).unwrap(), 74);
        assert_eq!(next_event_id_at(&sequence, &journal).unwrap(), 75);
    }

    #[test]
    fn corrupt_sequence_recovers_above_existing_journal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sequence = dir.path().join("events.seq");
        let journal = dir.path().join("events.jsonl");
        std::fs::write(&sequence, "76:broken").unwrap();
        std::fs::write(
            &journal,
            format!("{}\n", serde_json::to_string(&test_event(80)).unwrap()),
        )
        .unwrap();

        assert_eq!(next_event_id_at(&sequence, &journal).unwrap(), 81);
        assert_eq!(read_persisted_sequence(&sequence), Some(81));
    }

    #[test]
    fn concurrent_processes_allocate_unique_event_ids() {
        let dir = tempfile::tempdir().expect("temp dir");
        let executable = std::env::current_exe().expect("test executable");
        let mut children = Vec::new();
        for writer in 0..4 {
            children.push(
                std::process::Command::new(&executable)
                    .args([
                        "--exact",
                        "core::events::tests::persistent_event_id_writer_child",
                    ])
                    .env("__LEAN_CTX_EVENT_ID_TEST_DIR", dir.path())
                    .env("__LEAN_CTX_EVENT_ID_TEST_WRITER", writer.to_string())
                    .spawn()
                    .expect("spawn event id writer"),
            );
        }
        for mut child in children {
            assert!(child.wait().expect("wait for event id writer").success());
        }

        let mut ids = Vec::new();
        for writer in 0..4 {
            let content = std::fs::read_to_string(dir.path().join(format!("writer-{writer}.ids")))
                .expect("read allocated ids");
            ids.extend(content.lines().map(|line| line.parse::<u64>().unwrap()));
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 400, "every process must receive unique ids");
        assert_eq!(ids.first(), Some(&1));
        assert_eq!(ids.last(), Some(&400));
        let sequence_path = dir.path().join("events.seq");
        assert_eq!(read_persisted_sequence(&sequence_path), Some(400));
        assert_eq!(
            std::fs::read_to_string(sequence_path)
                .expect("read sequence journal")
                .lines()
                .count(),
            4,
            "four writers should persist four block reservations, not 400 ids"
        );
    }
}
