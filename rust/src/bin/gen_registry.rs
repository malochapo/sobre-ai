//! Canonicalize the bundled grammar registry (`rust/data/grammar_registry.json`).
//!
//! Run:   `cargo run --example gen_registry --features dev-tools`
//! Check: `cargo run --example gen_registry --features dev-tools -- --check`
//!
//! The registry is a generated snapshot: every entry is validated, entries are
//! sorted by name, and the JSON is written in one canonical form. CI runs
//! `--check` and fails on any byte drift — hand-edits must go through this
//! generator. `core::artifacts_exec::grammar_registry` owns the canonical form
//! and asserts the same invariant as a lib test.
//!
//! This tool also canonicalized `addon_registry.json` until the addon
//! marketplace was removed.

use std::path::{Path, PathBuf};

use lean_ctx::core::artifacts_exec::grammar_registry::{Snapshot, canonical_snapshot};

fn main() {
    let mut check_only = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--check" => check_only = true,
            "-h" | "--help" => {
                print_help();
                return;
            }
            other => {
                eprintln!("ERROR: unknown arg: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }

    let path = repo_data_dir().join("grammar_registry.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ERROR: read {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let snap: Snapshot = match canonical_snapshot(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    if text == snap.canonical {
        println!(
            "canonical {} ({} entries)",
            path.display(),
            snap.entry_count
        );
        return;
    }
    if check_only {
        eprintln!(
            "DRIFT: {} is not canonical — run: cargo run --example gen_registry \
             --features dev-tools",
            path.display()
        );
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(&path, &snap.canonical) {
        eprintln!("ERROR: write {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("wrote {} ({} entries)", path.display(), snap.entry_count);
}

/// `rust/data/`, resolved relative to this crate's manifest dir so the tool
/// works from any CWD.
fn repo_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn print_help() {
    println!(
        "gen_registry\n\n\
         USAGE:\n  cargo run --example gen_registry --features dev-tools [-- --check]\n\n\
         FILES:\n  rust/data/grammar_registry.json"
    );
}
