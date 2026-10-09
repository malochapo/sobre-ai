//! `sobre_knowledge recall`: bounded output, the same answer twice, and the
//! value a fact had at a past time (`as_of`).

use sobre::core::knowledge::ProjectKnowledge;
use sobre::core::memory_policy::MemoryPolicy;

#[test]
fn sobre_knowledge_recall_is_budgeted_and_deterministic() {
    let _g = sobre::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("SOBRE_DATA_DIR", data_dir.to_string_lossy().to_string()) };

    let project_root = tmp.path().join("proj");
    std::fs::create_dir_all(&project_root).expect("create project root");
    let project_root_str = project_root.to_string_lossy().to_string();

    let policy = MemoryPolicy::default();
    let mut knowledge = ProjectKnowledge::load_or_create(&project_root_str);
    for i in 0..50 {
        knowledge.remember(
            "architecture",
            &format!("k{i:02}"),
            &format!("v{i:02}"),
            "s1",
            0.8,
            &policy,
        );
    }
    knowledge.save().expect("save knowledge");

    let out1 = sobre::tools::sobre_knowledge::handle(
        &project_root_str,
        "recall",
        Some("architecture"),
        None,
        None,
        None,
        "s1",
        None,
        None,
        None,
    );
    let out2 = sobre::tools::sobre_knowledge::handle(
        &project_root_str,
        "recall",
        Some("architecture"),
        None,
        None,
        None,
        "s1",
        None,
        None,
        None,
    );

    assert_eq!(out1, out2, "recall output must be deterministic");
    assert!(
        out1.contains("showing 10/50"),
        "recall header must indicate truncation"
    );

    let fact_lines = out1
        .lines()
        .filter(|l| l.starts_with("  [architecture/"))
        .count();
    assert!(fact_lines <= 10, "must not exceed recall budget");

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::remove_var("SOBRE_DATA_DIR") };
}

#[test]
fn sobre_knowledge_recall_as_of_time_travels() {
    let _g = sobre::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("SOBRE_DATA_DIR", data_dir.to_string_lossy().to_string()) };

    let project_root = tmp.path().join("proj");
    std::fs::create_dir_all(&project_root).expect("create project root");
    let project_root_str = project_root.to_string_lossy().to_string();

    let policy = MemoryPolicy::default();
    let mut knowledge = ProjectKnowledge::load_or_create(&project_root_str);
    knowledge.remember("arch", "db", "PostgreSQL", "s1", 0.95, &policy);
    knowledge.facts[0].confirmation_count = 3;
    let between = chrono::Utc::now();
    std::thread::sleep(std::time::Duration::from_millis(10));
    knowledge.remember("arch", "db", "MySQL", "s2", 0.9, &policy);
    knowledge.save().expect("save knowledge");

    let past = sobre::tools::sobre_knowledge::handle(
        &project_root_str,
        "recall",
        None,
        None,
        None,
        Some("db"),
        "s1",
        None,
        None,
        Some(&between.to_rfc3339()),
    );
    assert!(
        past.contains("PostgreSQL") && !past.contains("MySQL"),
        "as_of recall must return the value valid at that time: {past}"
    );

    let invalid = sobre::tools::sobre_knowledge::handle(
        &project_root_str,
        "recall",
        None,
        None,
        None,
        Some("db"),
        "s1",
        None,
        None,
        Some("not-a-date"),
    );
    assert!(
        invalid.contains("invalid as_of"),
        "invalid as_of must produce a clear error: {invalid}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::remove_var("SOBRE_DATA_DIR") };
}
