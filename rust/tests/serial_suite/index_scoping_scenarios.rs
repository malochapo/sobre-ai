// ===== Group B: Index Status =====

#[test]
fn disk_status_shows_exists_false_for_nonexistent_project() {
    let ds = lean_ctx::core::index_orchestrator::disk_status("/nonexistent/project/xyz123");
    assert!(!ds.graph_index.exists);
    assert!(!ds.bm25_index.exists);
    assert!(!ds.code_graph.exists);
}

#[test]
fn status_json_includes_disk_section() {
    let json = lean_ctx::core::index_orchestrator::status_json("/tmp/test-status-json");
    let val: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(val.get("disk").is_some(), "status_json must include 'disk'");
    let disk = val.get("disk").unwrap();
    assert!(disk.get("graph_index").is_some());
    assert!(disk.get("bm25_index").is_some());
    assert!(disk.get("code_graph").is_some());
}

#[test]
fn disk_status_graph_index_not_built() {
    let ds = lean_ctx::core::index_orchestrator::disk_status("/tmp/no-such-project-12345");
    assert!(!ds.graph_index.exists);
    assert!(ds.graph_index.size_bytes.is_none());
    assert!(ds.graph_index.modified_at.is_none());
}
