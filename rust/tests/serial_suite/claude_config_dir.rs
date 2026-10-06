//! `CLAUDE_CONFIG_DIR` support: the `sobre` skill lands in the Claude config
//! directory the user chose.
//!
//! These tests modify process-global env vars — must run serialized.

use serial_test::serial;

fn with_claude_config_dir(value: Option<&str>, f: impl FnOnce()) {
    let prev = std::env::var("CLAUDE_CONFIG_DIR").ok();
    match value {
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        Some(v) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", v) },
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
    }
    f();
    match prev {
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        Some(v) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", v) },
        // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
        None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
    }
}

#[test]
#[serial]
fn claude_skill_defaults_to_dot_claude() {
    let home = std::path::Path::new("/h");
    with_claude_config_dir(None, || {
        assert_eq!(
            lean_ctx::setup::skill::skill_path("claude", home),
            Some(home.join(".claude/skills/sobre/SKILL.md"))
        );
    });
}

#[test]
#[serial]
fn claude_skill_follows_claude_config_dir() {
    let home = std::path::Path::new("/h");
    with_claude_config_dir(Some("/elsewhere/claude"), || {
        assert_eq!(
            lean_ctx::setup::skill::skill_path("claude", home),
            Some(std::path::PathBuf::from(
                "/elsewhere/claude/skills/sobre/SKILL.md"
            ))
        );
    });
}

#[test]
#[serial]
fn instructions_never_point_at_a_claude_file() {
    // The MCP instructions are the only rules channel; they must not send
    // agents looking for a CLAUDE.md that sobre no longer writes.
    with_claude_config_dir(Some("~/.arc/claude"), || {
        let instr = lean_ctx::instructions::build_instructions();
        assert!(!instr.contains("CLAUDE.md"), "{instr}");
        assert!(!instr.contains(".arc/claude"), "{instr}");
    });
}
