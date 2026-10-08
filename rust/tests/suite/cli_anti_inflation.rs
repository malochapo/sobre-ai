//! End-to-end anti-inflation guarantee for reads (#361). This exercises the
//! SHIPPED binary end-to-end (`lean-ctx call ctx_read`: resolver → frame → cap
//! → print) and proves that an `auto` read can never emit more tokens than the
//! bare file. It complements the in-process `cap_to_raw` unit tests.

use std::path::Path;
use std::process::Command;

use lean_ctx::core::tokens::count_tokens;

/// Runs an `auto` read of `file` through `lean-ctx call ctx_read` fully
/// hermetically and returns stdout.
///
/// Isolation: `HOME`/XDG point at empty temp dirs and `LEAN_CTX_DATA_DIR`
/// keeps stat tracking out of the real data dir.
fn read_auto(bin: &str, home: &Path, data_dir: &Path, file: &Path) -> String {
    let json = serde_json::json!({ "path": file, "mode": "auto" }).to_string();
    let out = Command::new(bin)
        .args(["call", "ctx_read", "--project-root"])
        .arg(file.parent().unwrap())
        .args(["--json", &json])
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("share"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("LEAN_CTX_DATA_DIR", data_dir)
        .output()
        .expect("run lean-ctx call ctx_read");
    assert!(
        out.status.success(),
        "read failed for {}: {}",
        file.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn auto_read_never_inflates_tiny_files() {
    let bin = env!("CARGO_BIN_EXE_lean-ctx");
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();

    // Tiny, incompressible files across the common types: framing alone would
    // exceed the content, so the cap is the load-bearing guarantee here.
    let cases = [
        ("tiny.json", "{\"a\":1}\n"),
        ("tiny.rs", "fn a() {}\n"),
        ("tiny.md", "# Hi\n"),
        ("tiny.txt", "hello world\n"),
        ("tiny.toml", "x = 1\n"),
        ("tiny.yaml", "a: 1\n"),
    ];

    for (name, content) in cases {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        let stdout = read_auto(bin, home.path(), data.path(), &path);

        let raw_tokens = count_tokens(content);
        // Trailing newline from `println!` is a display artifact, not payload.
        let emitted = count_tokens(stdout.trim_end_matches('\n'));
        assert!(
            emitted <= raw_tokens,
            "{name}: emitted {emitted} tok > raw {raw_tokens} tok (the read inflated it!)\n\
             --- stdout ---\n{stdout}"
        );
        // Capping must never drop data — the file body stays fully present.
        assert!(
            stdout.contains(content.trim_end()),
            "{name}: file content missing from output\n--- stdout ---\n{stdout}"
        );
    }
}
