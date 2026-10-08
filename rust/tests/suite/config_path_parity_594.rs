//! `LEAN_CTX_DATA_DIR` moves only the data dir, never config. These tests pin
//! that contract end-to-end by running the real binary with a controlled
//! environment and asserting the resolved `config.toml` path. `lean-ctx config
//! path` is the deterministic hook (no env mutation inside the test process, so
//! it is immune to parallel-test env races).
//!
//! Unix-only: `$HOME`/`$XDG_*` overrides + `dirs::home_dir()` honoring `HOME`.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

struct Sandbox {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
    xdg_config: std::path::PathBuf,
    xdg_data: std::path::PathBuf,
}

fn sandbox() -> Sandbox {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let xdg_config = home.join(".config");
    let xdg_data = home.join(".local/share");
    for d in [&home, &xdg_config, &xdg_data] {
        std::fs::create_dir_all(d).unwrap();
    }
    Sandbox {
        _tmp: tmp,
        home,
        xdg_config,
        xdg_data,
    }
}

/// Run `lean-ctx config path` in a fully controlled environment and return the
/// trimmed stdout (the resolved absolute `config.toml` path).
///
/// `data_dir` is an optional `LEAN_CTX_DATA_DIR` override. `cwd` is where the
/// process is launched. The layout-relevant `LEAN_CTX_*` vars are removed first
/// so host env never leaks into the assertion.
fn config_path(sb: &Sandbox, data_dir: Option<&Path>, cwd: &Path) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lean-ctx"));
    cmd.args(["config", "path"])
        .env("HOME", &sb.home)
        .env("XDG_CONFIG_HOME", &sb.xdg_config)
        .env("XDG_DATA_HOME", &sb.xdg_data)
        .env_remove("LEAN_CTX_CONFIG_DIR")
        .env_remove("LEAN_CTX_DATA_DIR")
        .env_remove("LEAN_CTX_STATE_DIR")
        .env_remove("LEAN_CTX_CACHE_DIR")
        .current_dir(cwd);
    if let Some(d) = data_dir {
        cmd.env("LEAN_CTX_DATA_DIR", d);
    }
    let out = cmd.output().expect("spawn lean-ctx config path");
    assert!(
        out.status.success(),
        "`lean-ctx config path` failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A `LEAN_CTX_DATA_DIR` override moves only the data dir: config stays where
/// the CLI resolves it, whatever the launch dir.
#[test]
fn custom_data_dir_does_not_move_config() {
    let sb = sandbox();

    let cli = config_path(&sb, None, &sb.home);
    let expected = sb.xdg_config.join("lean-ctx").join("config.toml");
    assert_eq!(
        cli,
        expected.to_string_lossy(),
        "terminal CLI must resolve the XDG config dir"
    );

    let custom = sb.home.join(".lean-ctx-custom");
    let pinned = config_path(&sb, Some(custom.as_path()), Path::new("/"));

    assert_eq!(pinned, cli, "a data-dir pin must not move config.toml");
}
