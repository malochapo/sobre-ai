#[path = "support/user_dirs.rs"]
mod user_dirs;

#[test]
fn harness_sandboxes_user_dirs() {
    user_dirs::assert_sandboxed();
}
