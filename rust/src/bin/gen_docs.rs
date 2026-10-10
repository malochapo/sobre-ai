//! Generate the code-derived reference pages under
//! `docs/reference/generated/`: MCP tools, config keys, the command line and
//! environment variables.
//!
//! Run:   `cargo run --example gen_docs --features dev-tools`
//! Check: `cargo run --example gen_docs --features dev-tools -- --check`
//!
//! CI runs `--check` to fail the build when a committed page differs from
//! what the code generates.

use std::path::{Path, PathBuf};

use clap::Parser;
use sobre::core::reference_docs;

/// Write the generated reference pages (default: <repo_root>/docs/reference/generated/).
#[derive(Parser)]
#[command(name = "gen_docs")]
struct Args {
    /// Write the pages here instead
    #[arg(long, value_name = "dir")]
    out_dir: Option<PathBuf>,
    /// Write nothing; exit 1 when a page is out of date
    #[arg(long)]
    check: bool,
}

fn main() {
    let args = Args::parse();
    let dir = args.out_dir.unwrap_or_else(reference_docs::generated_dir);
    let docs = reference_docs::generated_docs();

    if args.check {
        let mut stale = Vec::new();
        for (name, expected) in &docs {
            let path = dir.join(name);
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            if !reference_docs::content_matches(&on_disk, expected) {
                stale.push(path.display().to_string());
            }
        }
        if !stale.is_empty() {
            eprintln!(
                "Generated reference docs are out of date:\n  {}\n\nRun: cargo run --example gen_docs --features dev-tools\n",
                stale.join("\n  ")
            );
            std::process::exit(1);
        }
        return;
    }

    for (name, content) in &docs {
        let path = dir.join(name);
        match write_if_changed(&path, content) {
            Ok(changed) => {
                if changed {
                    println!("wrote {}", path.display());
                } else {
                    println!("unchanged {}", path.display());
                }
            }
            Err(e) => {
                eprintln!("ERROR: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn write_if_changed(path: &Path, content: &str) -> Result<bool, String> {
    if std::fs::read_to_string(path)
        .is_ok_and(|on_disk| reference_docs::content_matches(&on_disk, content))
    {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create_dir_all {}: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(true)
}
