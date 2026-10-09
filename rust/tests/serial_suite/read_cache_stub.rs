//! A re-read of an unchanged file answers with a short stub that says how to
//! get the content again.

use sobre::core::cache::SessionCache;
use sobre::core::protocol::CrpMode;

#[test]
fn unchanged_reread_is_a_stub_that_names_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test_file.py");
    std::fs::write(&file_path, "def hello():\n    return 'world'\n").unwrap();
    let path_str = file_path.to_str().unwrap();
    let mut cache = SessionCache::new();

    let first = sobre::tools::sobre_read::handle(&mut cache, path_str, "full", CrpMode::Off);
    assert!(
        first.contains("def hello():") && !first.contains("[unchanged"),
        "the first read delivers the content: {first}"
    );

    let second = sobre::tools::sobre_read::handle(&mut cache, path_str, "full", CrpMode::Off);
    assert!(
        second.contains("[unchanged") && second.contains("fresh=true"),
        "an unchanged re-read is a stub naming fresh=true: {second}"
    );
    assert!(
        !second.contains("def hello():"),
        "the stub does not repeat the content: {second}"
    );
}
