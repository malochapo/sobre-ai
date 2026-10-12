//! On-disk indexes: the BM25 cache, the semantic index and the graph
//! index's capacity.

use crate::doctor::{BOLD, DIM, GREEN, Outcome, RED, RST, YELLOW};

pub(crate) fn bm25_cache_health_outcome() -> Outcome {
    let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() else {
        return Outcome {
            ok: true,
            line: format!("{BOLD}BM25 cache{RST}  {DIM}skipped (no data dir){RST}"),
        };
    };

    let vectors_dir = data_dir.join("vectors");
    let Ok(entries) = std::fs::read_dir(&vectors_dir) else {
        return Outcome {
            ok: true,
            line: format!("{BOLD}BM25 cache{RST}  {GREEN}no vector dirs{RST}"),
        };
    };

    // Single source of truth with `save`/`load` (decoupled from the RAM profile;
    // see bm25_index::persist_ceiling_bytes) so the warning threshold here always
    // matches what is actually enforced on disk.
    let max_bytes = crate::core::bm25_index::persist_ceiling_bytes();
    let effective_mb = max_bytes / (1024 * 1024);
    let warn_bytes = max_bytes * 80 / 100; // 80% of effective limit
    let mut total_dirs = 0u32;
    let mut total_bytes = 0u64;
    let mut oversized: Vec<(String, u64)> = Vec::new();
    let mut warnings: Vec<(String, u64)> = Vec::new();
    let mut quarantined_count = 0u32;

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        total_dirs += 1;

        if dir.join("bm25_index.bin.zst.quarantined").exists() {
            quarantined_count += 1;
        }

        let index_path = dir.join("bm25_index.bin.zst");
        if let Ok(meta) = std::fs::metadata(&index_path) {
            let size = meta.len();
            total_bytes += size;
            let display = index_path.display().to_string();
            if size > max_bytes {
                oversized.push((display, size));
            } else if size > warn_bytes {
                warnings.push((display, size));
            }
        }
    }

    if !oversized.is_empty() {
        let details: Vec<String> = oversized
            .iter()
            .map(|(p, s)| format!("{p} ({:.1} GB)", *s as f64 / 1_073_741_824.0))
            .collect();
        return Outcome {
            ok: false,
            line: format!(
                "{BOLD}BM25 cache{RST}  {RED}{} index(es) exceed limit ({:.0} MB){RST}: {}  {DIM}(run: sobre doctor --fix){RST}",
                oversized.len(),
                max_bytes / (1024 * 1024),
                details.join(", ")
            ),
        };
    }

    if !warnings.is_empty() {
        let details: Vec<String> = warnings
            .iter()
            .map(|(p, s)| format!("{p} ({:.0} MB)", *s as f64 / 1_048_576.0))
            .collect();
        return Outcome {
            ok: true,
            line: format!(
                "{BOLD}BM25 cache{RST}  {YELLOW}{} index(es) >80% of {effective_mb} MB limit{RST}: {}  {DIM}(consider adding paths to `ignore`){RST}",
                warnings.len(),
                details.join(", ")
            ),
        };
    }

    let quarantine_note = if quarantined_count > 0 {
        format!("  {YELLOW}{quarantined_count} quarantined (run: sobre doctor --fix){RST}")
    } else {
        String::new()
    };

    Outcome {
        ok: true,
        line: format!(
            "{BOLD}BM25 cache{RST}  {GREEN}{total_dirs} index(es), {:.1} MB total{RST}{quarantine_note}",
            total_bytes as f64 / 1_048_576.0
        ),
    }
}
/// Runtime status of the semantic (BM25) index for the project around the
/// current directory: whether it is idle/building/ready/failed, how long the
/// last build took, and — crucially — *why* it might be stuck (e.g. "indexed
/// but NOT persisted: too large").
///
/// Users otherwise had no way to tell whether the semantic index
/// was working, how fast it was, or why it kept "warming up" forever.
pub(crate) fn semantic_index_outcome() -> Option<Outcome> {
    let project_root = current_project_root()?;

    let summary = crate::core::index_orchestrator::bm25_summary(&project_root);
    let disk = crate::core::index_orchestrator::disk_status(&project_root);
    let persisted = if disk.bm25_index.exists {
        match disk.bm25_index.size_bytes {
            Some(b) => format!("persisted {:.1} MB", b as f64 / 1_048_576.0),
            None => "persisted".to_string(),
        }
    } else {
        "not persisted".to_string()
    };

    let timing = match summary.elapsed_ms {
        Some(ms) if summary.state == "building" => format!(", {:.1}s elapsed", ms as f64 / 1000.0),
        Some(ms) => format!(", built in {:.1}s", ms as f64 / 1000.0),
        None => String::new(),
    };

    let outcome = match summary.state {
        "failed" => Outcome {
            ok: false,
            line: format!(
                "{BOLD}Semantic index{RST}  {RED}FAILED{RST}: {}  {DIM}(run: sobre index build){RST}",
                summary
                    .last_error
                    .or(summary.note)
                    .unwrap_or_else(|| "unknown error".to_string())
            ),
        },
        "building" => Outcome {
            ok: true,
            line: format!("{BOLD}Semantic index{RST}  {YELLOW}building{timing}{RST}"),
        },
        _ if summary
            .note
            .as_deref()
            .is_some_and(|n| n.contains("NOT persisted")) =>
        {
            Outcome {
                ok: false,
                line: format!(
                    "{BOLD}Semantic index{RST}  {YELLOW}rebuilds every cold start{RST}: {}",
                    summary.note.unwrap_or_default()
                ),
            }
        }
        "ready" => Outcome {
            ok: true,
            line: format!(
                "{BOLD}Semantic index{RST}  {GREEN}ready{RST} {DIM}({persisted}{timing}){RST}"
            ),
        },
        // idle: never asked to build this session — report disk state only.
        _ if disk.bm25_index.exists => Outcome {
            ok: true,
            line: format!(
                "{BOLD}Semantic index{RST}  {GREEN}ready{RST} {DIM}({persisted}, on disk){RST}"
            ),
        },
        _ => Outcome {
            ok: true,
            line: format!(
                "{BOLD}Semantic index{RST}  {DIM}not built yet (builds on first semantic search/compose){RST}"
            ),
        },
    };
    Some(outcome)
}
/// The project around the current directory, found as the other commands
/// find it; `None` for a broad root (`/`, the home directory), which is no
/// project.
fn current_project_root() -> Option<String> {
    let root = crate::cli::common::project_root(None);
    (!crate::core::pathutil::is_broad_or_unsafe_root(std::path::Path::new(&root))).then_some(root)
}

/// The current project's graph index against its file cap: critical from
/// 95%, a warning from 80%. `None` without a project or a graph index.
pub(crate) fn graph_capacity_outcome() -> Option<Outcome> {
    let graph_max_files = crate::core::config::GRAPH_INDEX_MAX_FILES;
    let project_root = current_project_root()?;
    let graph_files = crate::core::index_orchestrator::disk_status(&project_root)
        .graph_index
        .file_count?;
    let pct = (graph_files as f64 / graph_max_files as f64 * 100.0) as u32;
    let outcome = if pct >= 95 {
        Outcome {
            ok: false,
            line: format!(
                "{BOLD}Capacity [graph]{RST} {RED}CRIT: files {graph_files}/{graph_max_files} ({pct}%){RST}"
            ),
        }
    } else if pct >= 80 {
        Outcome {
            ok: true,
            line: format!(
                "{BOLD}Capacity [graph]{RST} {YELLOW}WARN: files {graph_files}/{graph_max_files} ({pct}%){RST}"
            ),
        }
    } else {
        Outcome {
            ok: true,
            line: format!(
                "{BOLD}Capacity [graph]{RST} {GREEN}files {graph_files}/{graph_max_files} ({pct}%){RST}"
            ),
        }
    };
    Some(outcome)
}
