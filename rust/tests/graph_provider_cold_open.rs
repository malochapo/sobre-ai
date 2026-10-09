//! The first *cold* `open_or_build` on a fresh project builds the graph and
//! returns it, rather than `None` and a "retry in a moment".
//!
//! In a binary of its own: the synchronous build gate is process-wide, so here
//! no other test can be holding it while this one waits.

#[path = "support/user_dirs.rs"]
mod user_dirs;

#[test]
fn harness_sandboxes_user_dirs() {
    user_dirs::assert_sandboxed();
}

#[test]
fn open_or_build_returns_graph_on_first_cold_open() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"cold_open\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn hello() -> &'static str { \"hi\" }\n",
    )
    .expect("write lib.rs");
    let root_str = root.to_string_lossy().to_string();

    // First call, no warm-up, no retry — must build synchronously.
    let provider = sobre::core::graph_provider::open_or_build(&root_str);
    assert!(
        provider.is_some(),
        "first cold open_or_build must build the graph synchronously"
    );
}
