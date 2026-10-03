#[path = "support/user_dirs.rs"]
mod user_dirs;

#[test]
fn harness_sandboxes_user_dirs() {
    user_dirs::assert_sandboxed();
}

#[cfg(feature = "embeddings")]
#[test]
fn knowledge_embeddings_semantic_search_must_include_expected() {
    use lean_ctx::core::knowledge_embedding::KnowledgeEmbeddingIndex;

    let idx = {
        let mut idx = KnowledgeEmbeddingIndex::new("projhash");
        idx.upsert("arch", "db", &[1.0, 0.0, 0.0]);
        idx.upsert("arch", "cache", &[0.0, 1.0, 0.0]);
        idx
    };

    let query = vec![1.0, 0.0, 0.0];
    let hits = idx.semantic_search(&query, 2);
    assert!(!hits.is_empty(), "expected at least one semantic hit");
    assert_eq!(hits[0].0.key, "db");
}
