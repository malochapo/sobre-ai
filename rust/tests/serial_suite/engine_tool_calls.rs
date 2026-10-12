//! Each tool called through the full MCP request pipeline of a
//! `ContextEngine`, on a small Rust project in a private data dir.
#![allow(clippy::await_holding_lock)]

use serde_json::json;

fn setup_project() -> (tempfile::TempDir, sobre::engine::ContextEngine) {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");

    std::fs::create_dir_all(project.join("src")).expect("create src");
    std::fs::write(
        project.join("src/main.rs"),
        r#"fn main() {
    let config = load_config();
    println!("Running with {:?}", config);
}

fn load_config() -> Config {
    Config { debug: false, port: 8080 }
}

#[derive(Debug)]
struct Config {
    debug: bool,
    port: u16,
}

fn helper_a() -> String {
    "hello".to_string()
}

fn helper_b(x: i32) -> i32 {
    x * 2
}
"#,
    )
    .expect("write main.rs");

    std::fs::write(
        project.join("src/lib.rs"),
        r#"pub mod utils;

pub fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#,
    )
    .expect("write lib.rs");

    std::fs::create_dir_all(project.join("src/utils")).expect("create utils");
    std::fs::write(
        project.join("src/utils/mod.rs"),
        r#"pub fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}
"#,
    )
    .expect("write utils/mod.rs");

    std::fs::write(
        project.join("Cargo.toml"),
        r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("write Cargo.toml");

    std::fs::write(
        project.join("README.md"),
        "# Test Project\nA sample project.\n",
    )
    .expect("write README.md");

    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::set_var("SOBRE_DATA_DIR", data_dir.to_string_lossy().to_string());
    }

    let engine = sobre::engine::ContextEngine::with_project_root(&project);
    (dir, engine)
}

// ── Tree ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tree_depth() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let tree = engine
        .call_tool_text("sobre_tree", Some(json!({"depth": 3})))
        .await
        .expect("tree");
    assert!(tree.contains("src"), "tree should show src: {tree}");
    assert!(
        tree.contains("Cargo.toml") || tree.contains("main.rs"),
        "tree should show files: {tree}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

// ── Read modes ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_full_and_cache_stub() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let project = _dir.path().join("project");
    let main_path = project.join("src/main.rs").to_string_lossy().to_string();

    let full1 = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "full"})),
        )
        .await
        .expect("full read 1");
    assert!(
        full1.contains("load_config"),
        "full read should contain fn body: {full1}"
    );

    let full2 = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "full"})),
        )
        .await
        .expect("full read 2");
    assert!(
        full2.contains("[unchanged"),
        "an unchanged re-read is a stub: {full2}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_signatures() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let project = _dir.path().join("project");
    let main_path = project.join("src/main.rs").to_string_lossy().to_string();

    let sigs = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "signatures"})),
        )
        .await
        .expect("signatures read");
    assert!(
        sigs.contains("fn main") || sigs.contains("load_config") || sigs.contains("helper_a"),
        "signatures should contain fn names: {sigs}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_map() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let project = _dir.path().join("project");
    let lib_path = project.join("src/lib.rs").to_string_lossy().to_string();

    let map = engine
        .call_tool_text("sobre_read", Some(json!({"path": lib_path, "mode": "map"})))
        .await
        .expect("map read");
    assert!(
        map.contains("greet") && map.contains("add"),
        "map should list the public functions: {map}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_lines_range() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let project = _dir.path().join("project");
    let main_path = project.join("src/main.rs").to_string_lossy().to_string();

    let lines = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "lines:1-5"})),
        )
        .await
        .expect("lines read");
    assert!(
        lines.contains("fn main"),
        "lines 1-5 should contain fn main: {lines}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_auto() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let project = _dir.path().join("project");
    let main_path = project.join("src/main.rs").to_string_lossy().to_string();

    let auto = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "auto"})),
        )
        .await
        .expect("auto read");
    assert!(auto.contains("load_config"), "auto read: {auto}");

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_aggressive() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let project = _dir.path().join("project");
    let main_path = project.join("src/main.rs").to_string_lossy().to_string();

    let aggressive = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "aggressive"})),
        )
        .await
        .expect("aggressive read");
    assert!(
        aggressive.contains("load_config"),
        "aggressive read: {aggressive}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_diff_after_edit() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let project = _dir.path().join("project");
    let main_path = project.join("src/main.rs").to_string_lossy().to_string();

    let _ = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "full"})),
        )
        .await
        .expect("initial read");

    let content = std::fs::read_to_string(project.join("src/main.rs")).unwrap();
    let modified = content.replace("debug: false", "debug: true");
    std::fs::write(project.join("src/main.rs"), &modified).unwrap();

    let diff = engine
        .call_tool_text(
            "sobre_read",
            Some(json!({"path": main_path, "mode": "diff"})),
        )
        .await
        .expect("diff read");
    assert!(
        diff.contains("debug: true"),
        "diff should show changes: {diff}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

// ── Search ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_regex() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let out = engine
        .call_tool_text("sobre_search", Some(json!({"pattern": "fn main"})))
        .await
        .expect("search");
    assert!(
        out.contains("main") || out.contains("src/main.rs"),
        "search should find fn main: {out}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_with_path_filter() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let out = engine
        .call_tool_text(
            "sobre_search",
            Some(json!({"pattern": "pub fn", "path": "src/utils"})),
        )
        .await
        .expect("search with path");
    assert!(
        out.contains("format_bytes"),
        "filtered search should find format_bytes: {out}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

// ── Shell ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_echo() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let out = engine
        .call_tool_text(
            "sobre_shell",
            Some(json!({"command": "echo hello-from-shell"})),
        )
        .await
        .expect("shell echo");
    assert!(
        out.contains("hello-from-shell"),
        "shell should contain echo output: {out}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_raw_mode() {
    let _lock = sobre::core::data_dir::test_env_lock();
    let (_dir, engine) = setup_project();

    let out = engine
        .call_tool_text(
            "sobre_shell",
            Some(json!({"command": "echo raw-test-value", "raw": true})),
        )
        .await
        .expect("shell raw");
    assert!(
        out.contains("raw-test-value"),
        "raw shell should contain output: {out}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe {
        std::env::remove_var("SOBRE_DATA_DIR");
    }
}
