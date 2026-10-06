use lean_ctx::core::archive;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Runs `f` with `LEAN_CTX_DATA_DIR` (which also holds `config.toml`) pointed
/// at a fresh temp dir whose config is `toml`, then restores the previous dir.
fn with_config<F: FnOnce()>(toml: &str, f: F) {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), toml).unwrap();
    let prev = std::env::var_os("LEAN_CTX_DATA_DIR");
    // SAFETY: serialized by this file's local `ENV_LOCK` mutex.
    unsafe { std::env::set_var("LEAN_CTX_DATA_DIR", dir.path()) };
    f();
    match prev {
        // SAFETY: serialized by this file's local `ENV_LOCK` mutex.
        Some(v) => unsafe { std::env::set_var("LEAN_CTX_DATA_DIR", v) },
        // SAFETY: serialized by this file's local `ENV_LOCK` mutex.
        None => unsafe { std::env::remove_var("LEAN_CTX_DATA_DIR") },
    }
}

fn with_test_dir<F: FnOnce()>(f: F) {
    with_config("", f);
}

#[test]
fn archive_store_and_retrieve() {
    with_test_dir(|| {
        let content = "Hello from archive test\nLine 2\nLine 3";
        let id = archive::store("ctx_shell", "echo test", content, None).unwrap();
        assert!(!id.is_empty());
        assert_eq!(archive::retrieve(&id).unwrap(), content);
    });
}

#[test]
fn archive_retrieve_range() {
    with_test_dir(|| {
        let content = (1..=20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let id = archive::store("ctx_read", "cat file", &content, None).unwrap();
        let range = archive::retrieve_with_range(&id, 5, 10).unwrap();
        assert!(range.contains("line 5"), "expected line 5 in: {range}");
        assert!(range.contains("line 10"), "expected line 10 in: {range}");
        assert!(
            !range.contains("line 4"),
            "should not contain line 4: {range}"
        );
        assert!(
            !range.contains("line 11"),
            "should not contain line 11: {range}"
        );
    });
}

#[test]
fn archive_idempotent() {
    with_test_dir(|| {
        let content = "same content for idempotency test";
        let id1 = archive::store("ctx_shell", "cmd1", content, None).unwrap();
        let id2 = archive::store("ctx_shell", "cmd2", content, None).unwrap();
        assert_eq!(id1, id2, "same content should produce same ID");
    });
}

#[test]
fn archive_session_filtering() {
    with_test_dir(|| {
        archive::store(
            "ctx_shell",
            "c1",
            "content-alpha-unique-test",
            Some("session-a"),
        )
        .unwrap();
        archive::store(
            "ctx_shell",
            "c2",
            "content-beta-unique-test",
            Some("session-b"),
        )
        .unwrap();
        archive::store(
            "ctx_read",
            "c3",
            "content-gamma-unique-test",
            Some("session-a"),
        )
        .unwrap();

        let all = archive::list_entries(None);
        assert_eq!(all.len(), 3);

        let sess_a = archive::list_entries(Some("session-a"));
        assert_eq!(sess_a.len(), 2);
        assert!(
            sess_a
                .iter()
                .all(|e| e.session_id.as_deref() == Some("session-a"))
        );
    });
}

#[test]
fn archive_cleanup_expired() {
    with_test_dir(|| {
        let id = archive::store("ctx_shell", "old", "old-content-for-cleanup-test", None).unwrap();
        let meta = archive::list_entries(None);
        assert_eq!(meta.len(), 1);

        // Manually backdate the entry
        let data_dir = std::env::var("LEAN_CTX_DATA_DIR").unwrap();
        let prefix = &id[..2];
        let meta_path = std::path::PathBuf::from(&data_dir)
            .join("archives")
            .join(prefix)
            .join(format!("{id}.meta.json"));
        let mut entry: archive::ArchiveEntry =
            serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
        entry.created_at = chrono::Utc::now() - chrono::Duration::hours(999);
        std::fs::write(&meta_path, serde_json::to_string(&entry).unwrap()).unwrap();

        let removed = archive::cleanup();
        assert!(removed >= 1, "expected cleanup to remove expired entry");
        assert!(
            archive::retrieve(&id).is_none(),
            "content should be gone after cleanup"
        );
    });
}

#[test]
fn archive_disk_usage_starts_zero() {
    with_test_dir(|| {
        assert_eq!(archive::disk_usage_bytes(), 0);
        archive::store("ctx_shell", "test", "some content for size", None).unwrap();
        assert!(archive::disk_usage_bytes() > 0);
    });
}

#[test]
fn ctx_expand_selectors_via_handle() {
    use lean_ctx::tools::ctx_expand;
    use serde_json::json;
    with_test_dir(|| {
        let content = (1..=40)
            .map(|i| format!("row {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let id = archive::store("ctx_shell", "run", &content, None).unwrap();

        let head = ctx_expand::handle(&json!({"id": id, "head": 5}));
        assert!(head.contains("row 1") && head.contains("row 5"));
        assert!(!head.contains("row 6"), "head leaked row 6: {head}");

        let tail = ctx_expand::handle(&json!({"id": id, "tail": 3}));
        assert!(tail.contains("row 38") && tail.contains("row 40"));

        let json_id = archive::store("ctx_shell", "api", r#"{"a":1,"b":[1,2,3]}"#, None).unwrap();
        let keys = ctx_expand::handle(&json!({"id": json_id, "json_keys": true}));
        assert!(keys.contains("object (2 keys)"), "got: {keys}");
        assert!(keys.contains("array(3)"), "got: {keys}");
    });
}
