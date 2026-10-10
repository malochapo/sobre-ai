//! A recall that finds nothing in the active facts brings archived ones back.

use sobre::core::knowledge::ProjectKnowledge;
use sobre::core::memory_policy::MemoryPolicy;

#[test]
fn recall_rehydrates_from_archive_when_active_set_empty() {
    let _g = sobre::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir");
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("SOBRE_DATA_DIR", data_dir.to_string_lossy().to_string()) };

    let project_root = tmp.path().join("proj");
    std::fs::create_dir_all(&project_root).expect("mkdir proj");
    let project_root_str = project_root.to_string_lossy().to_string();

    // Create a fact that will be archived by lifecycle (low confidence).
    let policy = MemoryPolicy::default();
    let mut k = ProjectKnowledge::new(&project_root_str);
    k.remember("architecture", "db", "PostgreSQL", "s1", 0.1, &policy);
    let _ = k.run_memory_lifecycle(&policy);
    k.save().expect("save");

    // Now the active set should be empty (fact archived), so recall should rehydrate it.
    let out = sobre::tools::sobre_knowledge::handle(
        &project_root_str,
        "recall",
        None,
        None,
        None,
        Some("db postgres"),
        "s1",
        None,
        None,
        None,
    );

    assert!(
        out.contains("architecture/db") || out.contains("PostgreSQL"),
        "expected rehydrated recall result, got: {out}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::remove_var("SOBRE_DATA_DIR") };
}
