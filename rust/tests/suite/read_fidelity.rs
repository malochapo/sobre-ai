//! #404 regression: a read must return file content byte-for-byte for
//! verbatim modes (`full`, `lines:`). The prose terse pipeline once ran on
//! read output and mangled source (dictionary substitutions, line-drop dedup),
//! breaking a `full` read's "complete content" contract.
//!
//! We spawn the real binary with the densest `compression_level` (so terse
//! *would* fire if it were allowed to) and call `ctx_read` through
//! `lean-ctx call`, so the assertion fails loudly on any regression.

use std::path::Path;
use std::process::{Command, Output};

/// Source laced with terse triggers: dictionary words (`execution`, `command`,
/// `return`), duplicate lines (line-drop dedup bait) and blank lines.
const SAMPLE: &str = "fn run() {\n\n    let command = execution();\n    let command = execution();\n    // command execution pipeline\n    return command;\n}\n";

/// `SAMPLE` in a project directory below the isolated `HOME` (`call` refuses
/// `HOME` itself as a project root).
fn sample_file(home: &Path) -> std::path::PathBuf {
    let proj = home.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let file = proj.join("sample.rs");
    std::fs::write(&file, SAMPLE).unwrap();
    file
}

fn read_output(home: &Path, file: &Path, mode: &str) -> Output {
    let data = home.join("data");
    std::fs::create_dir_all(&data).unwrap();
    // Force the densest compression so a missing guard would visibly mangle.
    std::fs::write(data.join("config.toml"), "compression_level = 0.7\n").unwrap();
    let json = serde_json::json!({ "path": file, "mode": mode, "fresh": true }).to_string();
    Command::new(env!("CARGO_BIN_EXE_lean-ctx"))
        .args(["call", "ctx_read", "--project-root"])
        .arg(file.parent().unwrap())
        .args(["--json", &json])
        .env("LEAN_CTX_DATA_DIR", &data)
        .env("HOME", home)
        .output()
        .expect("spawn lean-ctx call ctx_read")
}

#[test]
fn full_read_is_byte_exact() {
    let dir = tempfile::tempdir().unwrap();
    let file = sample_file(dir.path());

    let out = read_output(dir.path(), &file, "full");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(SAMPLE),
        "full read must contain verbatim source; got:\n{stdout}"
    );
}

#[test]
fn lines_read_is_byte_exact() {
    let dir = tempfile::tempdir().unwrap();
    let file = sample_file(dir.path());

    let out = read_output(dir.path(), &file, "lines:1-3");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The contract under test is fidelity: the SELECTED lines must survive
    // verbatim with no terse substitutions, regardless of how the range is
    // rendered. `lines:1-3` is windowed output, not a full-file dump, so the
    // oracle is the selected slice — not the whole SAMPLE (#684 fix made
    // `lines:` actually honour the window instead of returning the full file).
    let selected_lines = &SAMPLE.lines().collect::<Vec<_>>()[..3];
    for line in selected_lines {
        assert!(
            line.is_empty() || stdout.contains(line),
            "lines: read must not terse-mangle selected source; missing {line:?} in:\n{stdout}"
        );
    }
}
