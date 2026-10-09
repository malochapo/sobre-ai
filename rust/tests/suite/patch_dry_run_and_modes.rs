//! `sobre_patch` writes keep the file's mode, and a dry run checks everything
//! and writes nothing (audit B9, B10, B24).
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use sobre::core::cache::SessionCache;
use sobre::core::session::SessionState;
use sobre::server::tool_trait::{McpTool, ToolContext};
use sobre::tools::registered::sobre_patch::SobrePatchTool;
use tokio::sync::RwLock;

fn ctx_for(root: &Path, path: &Path) -> ToolContext {
    ToolContext {
        project_root: root.to_string_lossy().to_string(),
        resolved_paths: HashMap::from([("path".to_string(), path.to_string_lossy().to_string())]),
        cache: Some(Arc::new(RwLock::new(SessionCache::new()))),
        session: Some(Arc::new(RwLock::new(SessionState::new()))),
        ..ToolContext::default()
    }
}

fn args(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("expected an object, got {other}"),
    }
}

fn patch(args: &Map<String, Value>, ctx: &ToolContext) -> String {
    tokio::task::block_in_place(|| SobrePatchTool.handle(args, ctx))
        .unwrap_or_else(|e| panic!("sobre_patch rejected: {}", e.message))
        .text
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replace_all_keeps_the_file_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.sh");
    std::fs::write(&path, "echo a\necho a\n").unwrap();
    make_executable(&path);
    let ctx = ctx_for(dir.path(), &path);

    let out = patch(
        &args(json!({ "op": "replace_all", "path": path, "find": "a", "replace": "b" })),
        &ctx,
    );
    assert!(out.contains("2 replacements"), "{out}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "echo b\necho b\n");
    assert_eq!(mode(&path), 0o755, "replace_all must keep the file mode");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replace_symbol_keeps_the_file_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "fn a() -> i32 {\n    1\n}\n").unwrap();
    make_executable(&path);
    let ctx = ctx_for(dir.path(), &path);

    let out = patch(
        &args(json!({
            "op": "replace_symbol", "path": path, "line": 1, "end_line": 3,
            "new_text": "fn a() -> i32 {\n    2\n}"
        })),
        &ctx,
    );
    assert!(out.contains("replace_symbol_body applied"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn a() -> i32 {\n    2\n}\n"
    );
    assert_eq!(mode(&path), 0o755, "a symbol edit must keep the file mode");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_runs_show_the_diff_and_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    let original = "fn a() -> i32 {\n    1\n}\n";
    std::fs::write(&path, original).unwrap();
    let ctx = ctx_for(dir.path(), &path);
    let h1 = sobre::core::anchor::line_hash("fn a() -> i32 {");

    let calls = [
        json!({ "op": "set_line", "path": path, "line": 1, "hash": h1,
                "new_text": "fn b() -> i32 {", "dry_run": true }),
        json!({ "op": "replace_unique", "path": path, "old_text": "1",
                "new_text": "2", "dry_run": true }),
        json!({ "op": "replace_all", "path": path, "find": "1",
                "replace": "2", "dry_run": true }),
        json!({ "op": "replace_symbol", "path": path, "line": 1, "end_line": 3,
                "new_text": "fn a() -> i32 {\n    2\n}", "dry_run": true }),
    ];
    for call in calls {
        let out = patch(&args(call.clone()), &ctx);
        assert!(out.contains("DRY RUN, nothing written"), "{call}: {out}");
        assert!(out.contains("```diff"), "{call} must show its diff: {out}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "{call} must not write"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dry_run_still_reports_a_stale_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();
    let ctx = ctx_for(dir.path(), &path);

    let out = patch(
        &args(
            json!({ "op": "set_line", "path": path, "line": 1, "hash": "zzzz",
                      "new_text": "uno", "dry_run": true }),
        ),
        &ctx,
    );
    assert!(out.contains("CONFLICT:"), "{out}");
    assert!(!out.contains("DRY RUN"), "{out}");
}
