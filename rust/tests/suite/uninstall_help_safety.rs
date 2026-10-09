//! `sobre uninstall --help` (and `-h`) must print usage and remove
//! NOTHING, and so must any argument `uninstall` does not know: a typo such
//! as `--dryrun`, or an agent name as `init` takes, must never run a real,
//! irreversible uninstall.
//!
//! These run the real binary as a subprocess inside a fully isolated tempdir so
//! that — even if the guard ever regresses — no developer file is touched.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Output};

/// Isolated environment: HOME + all sobre dirs redirected into a tempdir,
/// with a marker file in the data dir whose survival proves "nothing removed".
struct Sandbox {
    _root: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
    marker: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let data = root.path().join("data");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let marker = data.join("marker.txt");
        std::fs::write(&marker, "do-not-delete").unwrap();
        Self {
            _root: root,
            home,
            data,
            marker,
        }
    }

    fn run_uninstall(&self, flag: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sobre"))
            .args(["uninstall", flag])
            .env("HOME", &self.home)
            .env("SOBRE_DATA_DIR", &self.data)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("SOBRE_CONFIG_DIR")
            .env_remove("SOBRE_STATE_DIR")
            .env_remove("SOBRE_CACHE_DIR")
            .output()
            .expect("spawn sobre uninstall")
    }
}

/// Removal log lines that must NEVER appear when only asking for help.
const REMOVAL_MARKERS: &[&str] = &[
    "Processes stopped",
    "Data directory removed",
    "Binary removed",
    "fully removed",
];

fn assert_is_help_not_uninstall(out: &Output, flag: &str, marker: &std::path::Path) {
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "`uninstall {flag}` should exit 0; got {}\nstdout:\n{stdout}",
        out.status
    );
    assert!(
        stdout.contains("USAGE") && stdout.contains("--keep-config"),
        "`uninstall {flag}` must print usage; stdout was:\n{stdout}"
    );
    for needle in REMOVAL_MARKERS {
        assert!(
            !stdout.contains(needle),
            "`uninstall {flag}` must not perform removal, but emitted {needle:?}:\n{stdout}"
        );
    }
    assert!(
        marker.exists(),
        "`uninstall {flag}` deleted the data dir marker — it ran a real uninstall!"
    );
}

#[test]
fn uninstall_long_help_shows_usage_and_removes_nothing() {
    let sandbox = Sandbox::new();
    let out = sandbox.run_uninstall("--help");
    assert_is_help_not_uninstall(&out, "--help", &sandbox.marker);
}

#[test]
fn uninstall_short_help_shows_usage_and_removes_nothing() {
    let sandbox = Sandbox::new();
    let out = sandbox.run_uninstall("-h");
    assert_is_help_not_uninstall(&out, "-h", &sandbox.marker);
}

#[test]
fn uninstall_unknown_arguments_exit_2_and_remove_nothing() {
    for arg in ["--dryrun", "--dry-run", "-n", "claude", "--bogus"] {
        let sandbox = Sandbox::new();
        let out = sandbox.run_uninstall(arg);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(2),
            "`uninstall {arg}` must exit 2\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stderr.contains(arg),
            "`uninstall {arg}` must name the argument it refused: {stderr}"
        );
        assert!(
            stdout.trim().is_empty(),
            "`uninstall {arg}` must not start: {stdout}"
        );
        assert!(
            sandbox.marker.exists(),
            "`uninstall {arg}` deleted the data dir marker — it ran a real uninstall!"
        );
    }
}
