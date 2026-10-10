//! A windowed `sobre_read(mode=anchored)`
//! keeps windowing after the same file is edited via `sobre_patch`.
use std::sync::Arc;

use serde_json::json;
use sobre::core::cache::SessionCache;
use sobre::core::session::SessionState;
use sobre::server::tool_trait::{McpTool, ToolContext};
use sobre::tools::registered::sobre_patch::SobrePatchTool;
use sobre::tools::registered::sobre_read::SobreReadTool;
use tokio::sync::RwLock;

fn ctx_for(root: &std::path::Path, file: &str) -> ToolContext {
    let cache = Arc::new(RwLock::new(SessionCache::new()));
    let session = Arc::new(RwLock::new(SessionState::new()));
    let mut resolved_paths = std::collections::HashMap::new();
    resolved_paths.insert("path".to_string(), file.to_string());
    ToolContext {
        project_root: root.to_string_lossy().to_string(),
        resolved_paths,
        crp_mode: sobre::tools::CrpMode::Off,
        cache: Some(cache),
        session: Some(session),
        path_errors: std::collections::HashMap::new(),
        bm25_cache: None,
        progress_sender: None,
        shell_cwd: None,
    }
}

/// Pull the `hash` for a 1-based line out of an anchored body (`N:hh|content`).
fn anchor_hash_for_line(anchored: &str, line: usize) -> String {
    let prefix = format!("{line}:");
    for l in anchored.lines() {
        if let Some(rest) = l.strip_prefix(&prefix)
            && let Some((hash, _)) = rest.split_once('|')
        {
            return hash.to_string();
        }
    }
    panic!("no anchor for line {line} in:\n{anchored}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anchored_window_survives_a_patch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grow.go");
    let mut body = String::new();
    for i in 1..=30 {
        use std::fmt::Write;
        let _ = writeln!(body, "func f{i}() {{}}");
    }
    std::fs::write(&path, format!("package main\n\n{body}")).unwrap();
    let p = path.to_string_lossy().to_string();
    let ctx = ctx_for(dir.path(), &p);

    let read_args = json!({ "path": p, "mode": "anchored", "start_line": 5, "limit": 3 })
        .as_object()
        .unwrap()
        .clone();

    // 1) anchored window before any edit — must window, and expose anchors.
    let before = tokio::task::block_in_place(|| SobreReadTool.handle(&read_args, &ctx)).unwrap();
    assert!(
        before.text.lines().count() < 8,
        "pre-edit anchored window must be small, got {} lines:\n{}",
        before.text.lines().count(),
        before.text
    );
    let hash5 = anchor_hash_for_line(&before.text, 5);

    // 2) anchored sobre_patch — marks the path `recently_edited` in the tracker.
    let patch = json!({
        "path": p,
        "op": "insert_after",
        "line": 5,
        "hash": hash5,
        "new_text": "func inserted() {}"
    })
    .as_object()
    .unwrap()
    .clone();
    let pr = tokio::task::block_in_place(|| SobrePatchTool.handle(&patch, &ctx)).unwrap();
    assert!(!pr.text.starts_with("ERROR"), "patch failed: {}", pr.text);

    // 3) anchored window after the edit — must still window.
    let after = tokio::task::block_in_place(|| SobreReadTool.handle(&read_args, &ctx)).unwrap();
    assert!(
        after.text.lines().count() < 8,
        "post-edit anchored window LEAKED the whole file: {} lines\n{}",
        after.text.lines().count(),
        after.text
    );
}
