//! On-disk stores & memory: BM25/semantic indexes, archive FTS, RAM
//! guardian, knowledge stores, capacity accounting.

#[allow(clippy::wildcard_imports)]
use crate::doctor::common::*;
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
/// Runtime status of the semantic (BM25) index for the active project: whether
/// it is idle/building/ready/failed, how long the last build took, and — crucially
/// — *why* it might be stuck (e.g. "indexed but NOT persisted: too large").
///
/// Users otherwise had no way to tell whether the semantic index
/// was working, how fast it was, or why it kept "warming up" forever.
pub(crate) fn semantic_index_outcome() -> Option<Outcome> {
    let session = crate::core::session::SessionState::load_latest()?;
    let project_root = session.project_root?;

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
/// Reports knowledge stores whose `project_root` was deleted (removed git
/// worktrees, thrown-away projects). Such a store can never be written again, so
/// its eviction cap can never self-heal — it is pure accumulated bloat. This is
/// informational (never a hard failure); `sobre doctor --fix` reclaims it.
pub(crate) fn orphaned_knowledge_outcome() -> Outcome {
    let orphans = crate::core::knowledge::maintenance::find_orphaned_stores();
    if orphans.is_empty() {
        return Outcome {
            ok: true,
            line: format!("{BOLD}Knowledge stores{RST}  {GREEN}no orphaned stores{RST}"),
        };
    }
    let bytes: u64 = orphans.iter().map(|o| o.size_bytes).sum();
    Outcome {
        ok: true,
        line: format!(
            "{BOLD}Knowledge stores{RST}  {YELLOW}{} orphaned ({} reclaimable){RST}  {DIM}(deleted projects — reclaim: sobre doctor --fix){RST}",
            orphans.len(),
            human_bytes(bytes)
        ),
    }
}
/// Classify one memory store against its cap: `None` below 80% or with no
/// cap, otherwise `(over_cap, "name: current/limit (pct%)")`. A store sitting
/// *at* its cap is healthy: eviction (run_lifecycle) keeps it there by design.
/// Only a store genuinely *over* cap is critical, which means eviction is not
/// keeping up.
pub(crate) fn capacity_level(name: &str, current: usize, limit: usize) -> Option<(bool, String)> {
    if limit == 0 {
        return None;
    }
    let pct = (current as f64 / limit as f64 * 100.0) as u32;
    let warning = format!("{name}: {current}/{limit} ({pct}%)");
    if pct > 100 {
        Some((true, warning))
    } else if pct >= 80 {
        Some((false, warning))
    } else {
        None
    }
}

pub(crate) fn capacity_warnings() -> Vec<Outcome> {
    let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() else {
        return vec![];
    };

    let policy = crate::core::memory_policy::MemoryPolicy::default();

    let knowledge_dir = data_dir.join("knowledge");
    let Ok(entries) = std::fs::read_dir(&knowledge_dir) else {
        return vec![Outcome {
            ok: true,
            line: format!("{BOLD}Capacity{RST} {GREEN}no memory stores{RST}"),
        }];
    };

    let mut results = Vec::new();

    for entry in entries.flatten() {
        let hash_dir = entry.path();
        if !hash_dir.is_dir() {
            continue;
        }
        let hash = hash_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let short_hash = &hash[..hash.len().min(8)];

        let mut checks: Vec<(String, usize, usize)> = Vec::new();
        let mut project_root = None;

        if let Ok(content) = std::fs::read_to_string(hash_dir.join("knowledge.json"))
            && let Ok(k) =
                serde_json::from_str::<crate::core::knowledge::ProjectKnowledge>(&content)
        {
            project_root = Some(k.project_root.clone());
            checks.push((
                "facts".to_string(),
                k.facts.len(),
                policy.knowledge.max_facts,
            ));
            checks.push((
                "patterns".to_string(),
                k.patterns.len(),
                policy.knowledge.max_patterns,
            ));
            checks.push((
                "history".to_string(),
                k.history.len(),
                policy.knowledge.max_history,
            ));
        }

        if let Ok(content) = std::fs::read_to_string(hash_dir.join("embeddings.json"))
            && let Ok(idx) = serde_json::from_str::<
                crate::core::knowledge_embedding::KnowledgeEmbeddingIndex,
            >(&content)
        {
            checks.push((
                "embeddings".to_string(),
                idx.entries.len(),
                policy.embeddings.max_facts,
            ));
        }

        if let Ok(content) = std::fs::read_to_string(hash_dir.join("gotchas.json"))
            && let Ok(g) =
                serde_json::from_str::<crate::core::gotcha_tracker::GotchaStore>(&content)
        {
            checks.push((
                "gotchas".to_string(),
                g.gotchas.len(),
                crate::core::gotcha_tracker::MAX_GOTCHAS,
            ));
        }

        let mut warnings: Vec<String> = Vec::new();
        let mut critical = false;

        for (name, current, limit) in &checks {
            if let Some((over_cap, warning)) = capacity_level(name, *current, *limit) {
                critical |= over_cap;
                warnings.push(warning);
            }
        }

        if !warnings.is_empty() {
            let color = if critical { RED } else { YELLOW };
            let label = if critical { "CRIT" } else { "WARN" };
            results.push(Outcome {
                ok: !critical,
                line: format!(
                    "{BOLD}Capacity [{short_hash}]{RST} {color}{label}: {}{RST}",
                    warnings.join(", ")
                ),
            });
            // Actionable guidance. A store *at* its cap is healthy by
            // design — eviction keeps it there; only *over* cap means curation is
            // falling behind. Point the operator at the right lever either way.
            results.push(Outcome {
                ok: true,
                line: format!(
                    "  {DIM}→ {}{RST}",
                    capacity_hint(critical, project_root.as_deref())
                ),
            });
        }
    }

    // Global checks (not per project hash)

    // Graph index file count vs limit
    let graph_max_files = crate::core::config::GRAPH_INDEX_MAX_FILES;
    if graph_max_files > 0
        && let Some(session) = crate::core::session::SessionState::load_latest()
        && let Some(ref project_root) = session.project_root
    {
        let disk_status = crate::core::index_orchestrator::disk_status(project_root);
        if let Some(graph_files) = disk_status.graph_index.file_count {
            let pct = (graph_files as f64 / graph_max_files as f64 * 100.0) as u32;
            if pct >= 95 {
                results.push(Outcome {
                            ok: false,
                            line: format!(
                                "{BOLD}Capacity [graph]{RST} {RED}CRIT: files {graph_files}/{graph_max_files} ({pct}%){RST}"
                            ),
                        });
            } else if pct >= 80 {
                results.push(Outcome {
                            ok: true,
                            line: format!(
                                "{BOLD}Capacity [graph]{RST} {YELLOW}WARN: files {graph_files}/{graph_max_files} ({pct}%){RST}"
                            ),
                        });
            }
        }
    }

    if results.is_empty() {
        results.push(Outcome {
            ok: true,
            line: format!("{BOLD}Capacity{RST} {GREEN}all stores within limits{RST}"),
        });
    }

    results
}
/// Actionable next-step for a memory capacity warning. Separated from the
/// on-disk capacity scan so the messaging is unit-tested directly: a store *at*
/// its cap is healthy (eviction holds it there), while *over* cap means curation
/// is behind and the operator should compact it.
pub(crate) fn capacity_hint(critical: bool, project_root: Option<&str>) -> String {
    if !critical {
        return "at/near cap is healthy by design — eviction holds the store at its cap (write-time dedup #970, 90-day prune #972)".to_string();
    }
    let root = project_root.unwrap_or("<project>");
    format!(
        "over cap — eviction is behind. Compact it now: sobre call sobre_knowledge --project-root '{root}' --json '{{\"action\":\"consolidate\"}}'"
    )
}
