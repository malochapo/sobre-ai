//! Scenario tests for an issue reported by `BazsiBazsi`: the cache-hit
//! message was misleading when a subagent had cached the file.

use lean_ctx::core::protocol::CrpMode;
use std::io::Write;

// ==========================================================================
// Cache-hit message is informative, not misleading
// ==========================================================================
// Bazsi's problem: "the cache even if invalidated sometimes returns that
// the content has already been cached, while i did not see the read"

#[test]
fn scenario_cache_hit_message_format() {
    use lean_ctx::core::cache::SessionCache;

    let mut cache = SessionCache::new();
    let dir = std::env::temp_dir().join("lean_ctx_bazsi_cache_test");
    let _ = std::fs::create_dir_all(&dir);
    let file_path = dir.join("test_file.py");
    {
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(f, "def hello():").unwrap();
        writeln!(f, "    return 'world'").unwrap();
    }
    let path_str = file_path.to_str().unwrap();

    // First read: should return full content
    let result1 = lean_ctx::tools::ctx_read::handle(&mut cache, path_str, "full", CrpMode::Off);
    assert!(
        !result1.contains("[unchanged"),
        "First read should NOT say unchanged: got {result1}"
    );
    assert!(
        result1.contains("hello") || result1.contains("def"),
        "First read should contain file content"
    );

    // Second read (cache hit): should use new message format
    let result2 = lean_ctx::tools::ctx_read::handle(&mut cache, path_str, "full", CrpMode::Off);
    assert!(
        result2.contains("[unchanged") || result2.contains("use cached context"),
        "Cache hit should use new message format: got {result2}"
    );
    // Must NOT say "Already in your context window"
    assert!(
        !result2.contains("Already in your context window"),
        "Old misleading message must not appear: got {result2}"
    );
    // Should show unchanged indicator or hint at fresh=true
    assert!(
        result2.contains("fresh=true")
            || result2.contains("cached context")
            || result2.contains("[unchanged"),
        "Should hint about fresh=true, cached context, or unchanged: got {result2}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
