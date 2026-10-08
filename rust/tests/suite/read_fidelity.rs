//! #404 regression: a read must return file content byte-for-byte for
//! verbatim modes (`full`, `lines:`). The prose terse pipeline once ran on
//! read output and mangled source (dictionary substitutions, line-drop dedup),
//! breaking a `full` read's "complete content" contract.
//!
//! We spawn the real binary with the densest `compression_level` (so terse
//! *would* fire if it were allowed to) and call `sobre_read` through
//! `sobre call`, so the assertion fails loudly on any regression.

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
    Command::new(env!("CARGO_BIN_EXE_sobre"))
        .args(["call", "sobre_read", "--project-root"])
        .arg(file.parent().unwrap())
        .args(["--json", &json])
        .env("SOBRE_DATA_DIR", &data)
        .env("HOME", home)
        .output()
        .expect("spawn sobre call sobre_read")
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

fn call_read(home: &Path, project: &Path, args: &serde_json::Value) -> Output {
    let data = home.join("data");
    std::fs::create_dir_all(&data).unwrap();
    Command::new(env!("CARGO_BIN_EXE_sobre"))
        .args(["call", "sobre_read", "--project-root"])
        .arg(project)
        .args(["--json", &args.to_string()])
        .env("SOBRE_DATA_DIR", &data)
        .env("HOME", home)
        .output()
        .expect("spawn sobre call sobre_read")
}

#[test]
fn raw_is_the_bytes_on_disk_and_full_labels_redactions() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let file = proj.join("settings.py");
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let text = format!("TOKEN = \"{secret}\"\n");
    std::fs::write(&file, &text).unwrap();

    let raw = call_read(
        dir.path(),
        &proj,
        &serde_json::json!({ "path": file, "raw": true }),
    );
    assert_eq!(
        String::from_utf8_lossy(&raw.stdout).trim_end(),
        text.trim_end()
    );

    let full = call_read(
        dir.path(),
        &proj,
        &serde_json::json!({ "path": file, "mode": "full", "fresh": true }),
    );
    let full = String::from_utf8_lossy(&full.stdout);
    assert!(full.contains("[REDACTED:"), "{full}");
    assert!(!full.contains(secret), "{full}");
}

#[test]
fn an_unknown_mode_is_an_error_not_the_whole_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = sample_file(dir.path());
    let out = call_read(
        dir.path(),
        file.parent().unwrap(),
        &serde_json::json!({ "path": file, "mode": "bogus" }),
    );
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("execution()"), "{stdout}");
    let all = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(all.contains("invalid read mode"), "{all}");
}

#[test]
fn a_mode_picked_for_an_instruction_file_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let file = proj.join("AGENTS.md");
    std::fs::write(&file, "# Rules\nline two\nline three\n").unwrap();
    let out = call_read(
        dir.path(),
        &proj,
        &serde_json::json!({ "path": file, "mode": "lines:2-2", "fresh": true }),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("line two"), "{stdout}");
    assert!(!stdout.contains("line three"), "{stdout}");
}

#[test]
fn a_file_over_the_read_limit_is_read_in_windows() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let file = proj.join("big.log");
    let line = "x".repeat(99);
    let body = (1..=50_000).fold(String::new(), |mut body, i| {
        use std::fmt::Write as _;
        let _ = writeln!(body, "{i:05} {line}");
        body
    });
    std::fs::write(&file, body).unwrap();
    assert!(std::fs::metadata(&file).unwrap().len() > 4 * 1024 * 1024);

    let window = call_read(
        dir.path(),
        &proj,
        &serde_json::json!({ "path": file, "mode": "lines:3-4" }),
    );
    assert!(window.status.success(), "{window:?}");
    let stdout = String::from_utf8_lossy(&window.stdout);
    assert!(
        stdout.contains("00003 ") && stdout.contains("00004 "),
        "{stdout}"
    );
    assert!(!stdout.contains("00005 "), "{stdout}");

    let whole = call_read(
        dir.path(),
        &proj,
        &serde_json::json!({ "path": file, "mode": "full" }),
    );
    assert!(!whole.status.success());
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&whole.stdout),
        String::from_utf8_lossy(&whole.stderr)
    );
    assert!(all.contains("lines:1-200"), "{all}");
}
