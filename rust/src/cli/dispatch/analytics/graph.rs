//! `lean-ctx graph|compact` — the code-graph command surface and
//! transcript compaction.

use crate::{core, tools};

pub(in crate::cli::dispatch) fn cmd_graph(rest: &[String]) {
    // `--json` is positional-agnostic: strip it out and remember the choice so
    // positional parsing below stays simple.
    let want_json = rest.iter().any(|a| a == "--json");
    let fmt = if want_json { Some("json") } else { None };
    let filtered: Vec<String> = rest
        .iter()
        .filter(|a| a.as_str() != "--json")
        .cloned()
        .collect();
    let rest = &filtered[..];
    let sub = rest.first().map_or("build", std::string::String::as_str);
    match sub {
        "build" => {
            // `--force`/`-f` purges the cache for a from-scratch rebuild. A plain
            // `graph build` always rescans (incremental, hash-reused) so it reflects
            // the current source — `load_or_build` can otherwise return a cached
            // index until the staleness TTL elapses.
            let force = rest.iter().any(|a| a == "--force" || a == "-f");
            let root = rest
                .iter()
                .skip(1)
                .find(|a| !a.starts_with('-'))
                .cloned()
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| ".".to_string());
            if force {
                core::graph_index::purge_index(&root);
            }
            let index = core::graph_index::scan(&root);
            println!(
                "Graph built: {} files, {} edges",
                index.files.len(),
                index.edges.len()
            );
        }
        "related" | "impact" | "symbol" | "status" | "neighbors" => {
            let path_arg = if sub == "status" {
                None
            } else {
                rest.get(1).map(String::as_str)
            };
            let root_idx = if sub == "status" { 1 } else { 2 };
            let root = resolve_graph_root(rest.get(root_idx));
            println!(
                "{}",
                tools::ctx_graph::handle(
                    sub,
                    path_arg,
                    &root,
                    &mut core::cache::SessionCache::new(),
                    tools::CrpMode::Off,
                    None,
                    None,
                    None,
                    fmt,
                )
            );
        }
        "path" => {
            let from = rest.get(1).map(String::as_str);
            let to = rest.get(2).map(String::as_str);
            let root = resolve_graph_root(rest.get(3));
            println!(
                "{}",
                tools::ctx_graph::handle(
                    sub,
                    from,
                    &root,
                    &mut core::cache::SessionCache::new(),
                    tools::CrpMode::Off,
                    None,
                    None,
                    to,
                    fmt,
                )
            );
        }
        _ => {
            eprintln!(
                "Usage:\n  \
                 lean-ctx graph build [path]\n  \
                 lean-ctx graph related <file>\n  \
                 lean-ctx graph impact <file|symbol>\n  \
                 lean-ctx graph symbol <name>\n  \
                 lean-ctx graph neighbors <file> [--json]\n  \
                 lean-ctx graph path <from> <to> [--json]\n  \
                 lean-ctx graph status"
            );
            std::process::exit(1);
        }
    }
}

fn resolve_graph_root(arg: Option<&String>) -> String {
    arg.cloned()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| ".".to_string())
}

pub(in crate::cli::dispatch) fn cmd_compact(rest: &[String]) {
    let target = rest.first().map_or_else(
        || {
            let home = dirs::home_dir().unwrap_or_default();
            let claude = home.join(".claude").join("projects");
            if claude.is_dir() {
                claude
            } else {
                let cursor = home.join(".cursor").join("agent-transcripts");
                if cursor.is_dir() {
                    cursor
                } else {
                    std::env::current_dir().unwrap_or_default()
                }
            }
        },
        std::path::PathBuf::from,
    );

    if !target.exists() {
        eprintln!("Path does not exist: {}", target.display());
        std::process::exit(1);
    }

    let result = if target.is_file() {
        core::transcript_compact::compact_file(&target)
    } else {
        core::transcript_compact::compact_directory(&target)
    };

    match result {
        Ok(stats) => {
            println!("Transcript compaction: {stats}");
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
