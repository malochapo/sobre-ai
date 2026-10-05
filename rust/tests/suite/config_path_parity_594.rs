//! End-to-end regression for GitHub #594:
//! "LeanCTX config path is different for CLI and MCP".
//!
//! Older lean-ctx versions baked `LEAN_CTX_DATA_DIR` into the editor's MCP `env`
//! block. That collapsed config/state/cache onto the data dir *for the MCP
//! server only*, so the editor-spawned server read `config.toml` from
//! `~/.local/share/lean-ctx` while the terminal CLI read it from
//! `~/.config/lean-ctx` — the two silently diverged.
//!
//! `LEAN_CTX_DATA_DIR` now moves only the data dir, never config. These
//! tests pin that contract end-to-end by running the real binary twice with a
//! controlled environment (terminal vs. simulated editor-MCP) and asserting the
//! resolved `config.toml` path is identical. `lean-ctx config path` is the
//! deterministic hook (no env mutation inside the test process, so it is immune
//! to parallel-test env races).
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
/// `data_dir` simulates the `LEAN_CTX_DATA_DIR` an editor may have baked into the
/// MCP entry (`None` = a plain terminal). `cwd` simulates where the process is
/// launched (a terminal in `$HOME` vs. an editor-spawned MCP server at `/`). The
/// layout-relevant `LEAN_CTX_*` vars are removed first so host env never leaks
/// into the assertion.
fn config_path(sb: &Sandbox, data_dir: Option<&Path>, cwd: &Path) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lean-ctx"));
    cmd.args(["config", "path"])
        .env("HOME", &sb.home)
        .env("XDG_CONFIG_HOME", &sb.xdg_config)
        .env("XDG_DATA_HOME", &sb.xdg_data)
        .env("LEAN_CTX_ACTIVE", "1")
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

/// The reported bug: the terminal CLI and the editor-spawned MCP server must
/// resolve the SAME `config.toml`, even when the MCP env carries the standard
/// `LEAN_CTX_DATA_DIR` pin and the server is launched at `/`.
#[test]
fn cli_and_mcp_resolve_same_config_path_with_standard_data_pin() {
    let sb = sandbox();

    let cli = config_path(&sb, None, &sb.home);

    let standard_data = sb.xdg_data.join("lean-ctx");
    let mcp = config_path(&sb, Some(standard_data.as_path()), Path::new("/"));

    let expected = sb.xdg_config.join("lean-ctx").join("config.toml");
    assert_eq!(
        cli,
        expected.to_string_lossy(),
        "terminal CLI must resolve the XDG config dir"
    );
    assert_eq!(
        mcp, cli,
        "GH #594: a standard LEAN_CTX_DATA_DIR pin must NOT make the MCP server \
         diverge from the CLI"
    );
}

/// A *custom* `LEAN_CTX_DATA_DIR` moves only the data dir: config stays where
/// the CLI resolves it.
#[test]
fn custom_data_dir_does_not_move_config() {
    let sb = sandbox();

    let cli = config_path(&sb, None, &sb.home);

    let custom = sb.home.join(".lean-ctx-custom");
    let pinned = config_path(&sb, Some(custom.as_path()), Path::new("/"));

    assert_eq!(pinned, cli, "a data-dir pin must not move config.toml");
}
