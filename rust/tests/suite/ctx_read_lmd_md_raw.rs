//! `.lmd.md` reads are raw — like any other file.
//!
//! After the lmd reverse-cut, sobre has no `.lmd.md`-specific code path: a
//! read returns the raw source verbatim (never an error, never a half-rendered
//! document). Rendering `.lmd.md` is owned entirely by the external lean-md
//! addon (`ctx_md_render` / CLI `lean-md render`) and is out of scope here.
//! This end-to-end check drives the freshly built `sobre` binary and asserts
//! a `.lmd.md` read surfaces the raw marker.
use std::process::Command;

/// The sobre binary under test — the freshly built one, never the (possibly
/// stale) `sobre` on PATH.
const SOBRE_BIN: &str = env!("CARGO_BIN_EXE_sobre");

#[test]
fn ctx_read_lmd_md_returns_raw_source() {
    // No addon installed (default CI state) → a read of a `.lmd.md` must surface
    // the raw source, never an error or a half-rendered document. We assert the
    // marker survives the read.
    //
    // Hermetic isolation: the spawned binary gets a fresh, private data dir and
    // the read is `fresh`, so it never returns an `[unchanged …]` cache stub
    // seeded by a prior run, and never touches the real store.
    let fixture = tempfile::tempdir().expect("fixture dir");
    let data_dir = tempfile::tempdir().expect("isolated SOBRE_DATA_DIR");
    let f = fixture.path().join("d.lmd.md");
    std::fs::write(&f, "@date\nRAW_DELEGATION_MARKER\n").unwrap();

    let json = serde_json::json!({ "path": f, "mode": "full", "fresh": true }).to_string();
    let out = Command::new(SOBRE_BIN)
        .env("SOBRE_DATA_DIR", data_dir.path())
        .args(["call", "ctx_read", "--project-root"])
        .arg(fixture.path())
        .args(["--json", &json])
        .output()
        .expect("sobre call ctx_read");
    let text = String::from_utf8_lossy(&out.stdout);

    assert!(
        text.contains("RAW_DELEGATION_MARKER"),
        "without an addon a .lmd.md read must return raw text (no error, no half-render): {text}"
    );
    // The `@date` directive is what discriminates raw from rendered: any renderer
    // consumes it and substitutes a date. Asserting on the plain marker alone would
    // survive a re-introduced render pass, so this is the line that makes the gate bite.
    assert!(
        text.contains("@date"),
        "a rendered .lmd.md would have consumed the @date directive; the read must be raw: {text}"
    );
}
