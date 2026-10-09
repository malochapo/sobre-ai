use serde_json::json;

/// Calls `tool` through the full MCP request pipeline and returns its text.
async fn call_tool(
    engine: &sobre::engine::ContextEngine,
    tool: &str,
    arguments: serde_json::Value,
) -> String {
    engine
        .call_tool_text(tool, Some(arguments))
        .await
        .unwrap_or_else(|e| panic!("{tool} failed: {e:#}"))
}

/// Poll `cond` up to ~10s. The memory stores are persisted on a background
/// thread (the consolidation pipeline), so asserting immediately after a tool
/// call returns raced the writer and made this test flaky under full-suite load.
/// Polling keeps the assertion strict (it still fails if the artifact
/// never lands) while tolerating scheduling latency.
async fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if cond() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    cond()
}

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_benchmark_suite_persists_core_artifacts() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let data_dir = tempfile::tempdir().expect("data dir");
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("SOBRE_DATA_DIR", data_dir.path()) };

    let dir = tempfile::tempdir().expect("project dir");
    // Ensure project-root detection can anchor on a marker.
    std::fs::create_dir_all(dir.path().join(".git")).expect("create .git marker");
    let file_path = dir.path().join("a.txt");
    std::fs::write(&file_path, "hello\n").expect("write file");

    let root = sobre::core::pathutil::safe_canonicalize_or_self(dir.path());
    let root_str = root.to_string_lossy().to_string();
    let engine = sobre::engine::ContextEngine::with_project_root(&root);

    // 1) Establish project context (tool pipeline).
    let _ = call_tool(
        &engine,
        "sobre_read",
        json!({
            "path": file_path.to_string_lossy().to_string(),
            "mode": "full"
        }),
    )
    .await;

    // 2) Knowledge: remember.
    let _ = call_tool(
        &engine,
        "sobre_knowledge",
        json!({
            "action": "remember",
            "category": "arch",
            "key": "db",
            "value": "MySQL",
            "confidence": 0.9
        }),
    )
    .await;
    let _ = call_tool(
        &engine,
        "sobre_knowledge",
        json!({
            "action": "remember",
            "category": "arch",
            "key": "cache",
            "value": "Redis",
            "confidence": 0.9
        }),
    )
    .await;
    // 3) Assert artifacts exist on disk. Persistence runs on a background thread,
    //    so poll with a bounded timeout rather than asserting immediately.
    let knowledge = sobre::core::knowledge::ProjectKnowledge::load_or_create(&root_str);
    let knowledge_dir = data_dir
        .path()
        .join("knowledge")
        .join(&knowledge.project_hash);

    assert!(
        wait_until(|| knowledge_dir.join("knowledge.json").exists()).await,
        "expected knowledge.json to exist"
    );
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::remove_var("SOBRE_DATA_DIR") };
}
