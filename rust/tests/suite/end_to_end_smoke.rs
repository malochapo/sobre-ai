//! Smoke test: `sobre config` lists the keys.

use std::process::Command;

// ─── helpers ───────────────────────────────────────────────────────

fn sobre_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sobre"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd
}

mod cli_commands {
    use super::*;

    #[test]
    fn cli_config_lists_the_keys() {
        let out = sobre_bin().arg("config").output().unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("passthrough_commands"));
    }
}
