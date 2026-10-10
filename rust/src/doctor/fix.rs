use chrono::Utc;

use super::{compact_score, display_user_path, print_compact_status};

pub(super) struct DoctorFixOptions {
    pub json: bool,
}

pub(super) fn run_fix(opts: &DoctorFixOptions) -> Result<i32, String> {
    let report = build_fix_report(opts.json)?;
    if opts.json {
        let json_text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        crate::outln!("{json_text}");
    } else {
        print_repairs(&report);
        let (passed, total) = compact_score();
        print_compact_status(passed, total);
    }
    Ok(i32::from(!report.success))
}

/// Repairs without printing anything: what `doctor integrations --fix
/// --json` runs before its own report.
pub(super) fn run_fix_silently() {
    let _ = build_fix_report(true);
}

/// What `--fix` changed, one line each; what needed nothing is left out.
fn print_repairs(report: &crate::core::setup_report::SetupReport) {
    let mut changed = 0;
    for step in &report.steps {
        let label = match step.name.as_str() {
            "agents" => None,
            "bm25_cache_prune" => Some("Search indexes"),
            "graph_cache_prune" => Some("Code graphs"),
            "proxy_env" => Some("Proxy URLs"),
            "crash_loop_reset" => Some("MCP start history"),
            _ => continue,
        };
        for item in &step.items {
            if matches!(item.status.as_str(), "already" | "clean" | "no stale URLs") {
                continue;
            }
            changed += 1;
            let what = label.unwrap_or(&item.name);
            match &item.note {
                Some(note) => crate::outln!("  {what}: {} ({note})", item.status),
                None => crate::outln!("  {what}: {}", item.status),
            }
        }
        for warning in &step.warnings {
            crate::outln!("  note: {warning}");
        }
        for error in &step.errors {
            crate::outln!("  error: {error}");
        }
    }
    if changed == 0 {
        crate::outln!("  Nothing needed repair.");
    }
}

fn build_fix_report(quiet: bool) -> Result<crate::core::setup_report::SetupReport, String> {
    use crate::core::setup_report::{PlatformInfo, SetupItem, SetupReport, SetupStepReport};

    let _quiet_guard = quiet.then(crate::core::runtime_flags::scoped_quiet);
    let started_at = Utc::now();
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;

    let mut steps: Vec<SetupStepReport> = Vec::new();

    // The connection `init` runs (MCP entry, hooks and skill), for the
    // agents that already have sobre only: connecting another is `init`'s
    // job.
    let mut agents_step = SetupStepReport {
        name: "agents".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let connected = crate::setup::agents_with_sobre(&home);
    let connected: Vec<&str> = connected.iter().map(String::as_str).collect();
    let outcomes = if connected.is_empty() {
        Vec::new()
    } else {
        crate::setup::connect_agents(&connected, false)
    };
    for outcome in outcomes {
        if !outcome.detected {
            continue;
        }
        let status = if !outcome.errors.is_empty() {
            "error"
        } else if outcome.files.is_empty() {
            "already"
        } else {
            "repaired"
        };
        agents_step.ok &= outcome.errors.is_empty();
        agents_step.items.push(SetupItem {
            name: outcome.name.to_string(),
            status: status.to_string(),
            path: None,
            note: (!outcome.files.is_empty()).then(|| {
                outcome
                    .files
                    .iter()
                    .map(|(p, _)| crate::setup::display_path(p, &home))
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        });
        agents_step.warnings.extend(
            outcome
                .warnings
                .iter()
                .map(|w| format!("{}: {w}", outcome.name)),
        );
        agents_step.errors.extend(
            outcome
                .errors
                .iter()
                .map(|e| format!("{}: {e}", outcome.name)),
        );
    }
    if agents_step.items.is_empty() {
        agents_step
            .warnings
            .push("no agent has sobre yet; connect one with `sobre init`".to_string());
    }
    steps.push(agents_step);

    for (name, prune_result) in [
        ("bm25_cache_prune", prune_bm25_caches()),
        ("graph_cache_prune", prune_graph_caches()),
    ] {
        steps.push(SetupStepReport {
            name: name.to_string(),
            ok: true,
            items: vec![SetupItem {
                name: "prune".to_string(),
                status: if prune_result.removed > 0 {
                    "pruned".to_string()
                } else {
                    "clean".to_string()
                },
                path: None,
                note: Some(format!(
                    "scanned {}, removed {}, freed {:.1} MB",
                    prune_result.scanned,
                    prune_result.removed,
                    prune_result.bytes_freed as f64 / 1_048_576.0
                )),
            }],
            warnings: Vec::new(),
            errors: Vec::new(),
        });
    }

    let mut proxy_env_step = SetupStepReport {
        name: "proxy_env".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let cleaned = crate::proxy_setup::cleanup_stale_proxy_env(&home);
    proxy_env_step.items.push(SetupItem {
        name: "stale_proxy_urls".to_string(),
        status: if cleaned > 0 {
            format!("cleaned {cleaned} stale URL(s)")
        } else {
            "no stale URLs".to_string()
        },
        path: None,
        note: None,
    });
    steps.push(proxy_env_step);

    let mut startup_step = SetupStepReport {
        name: "crash_loop_reset".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    crate::core::startup_guard::reset_crash_loop(crate::core::startup_guard::MCP_PROCESS_NAME);
    startup_step.items.push(SetupItem {
        name: "crash_loop_backoff".to_string(),
        status: "reset".to_string(),
        path: None,
        note: Some("so a restart loop no longer delays the next start".to_string()),
    });
    steps.push(startup_step);

    steps.push(orphaned_knowledge_step());

    let mut verify_step = SetupStepReport {
        name: "verify".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let (passed, total) = compact_score();
    verify_step.items.push(SetupItem {
        name: "doctor_compact".to_string(),
        status: format!("{passed}/{total}"),
        path: None,
        note: None,
    });
    if passed != total {
        verify_step.warnings.push(format!(
            "doctor compact not fully passing: {passed}/{total}"
        ));
    }
    steps.push(verify_step);

    let finished_at = Utc::now();
    let success = steps.iter().all(|s| s.ok);

    let report = SetupReport {
        schema_version: 1,
        started_at,
        finished_at,
        success,
        dry_run: false,
        platform: PlatformInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        },
        steps,
        warnings: Vec::new(),
        errors: Vec::new(),
    };

    Ok(report)
}

/// Knowledge stores whose project root is not on disk, listed and never
/// deleted: the project may have moved, or sit on an unmounted drive.
fn orphaned_knowledge_step() -> crate::core::setup_report::SetupStepReport {
    let warnings = crate::core::knowledge::maintenance::find_orphaned_stores()
        .into_iter()
        .map(|orphan| {
            format!(
                "kept knowledge store {} ({}): its project is not on disk (moved, deleted, or \
                 on an unmounted drive); delete it by hand if the project is gone",
                display_user_path(&orphan.dir),
                super::common::human_bytes(orphan.size_bytes)
            )
        })
        .collect();
    crate::core::setup_report::SetupStepReport {
        name: "orphaned_knowledge".to_string(),
        ok: true,
        items: Vec::new(),
        warnings,
        errors: Vec::new(),
    }
}

pub(super) struct PruneResult {
    pub scanned: u32,
    pub removed: u32,
    pub bytes_freed: u64,
}

/// Drop BM25 indexes that are quarantined, over the persist cap, or belong to
/// a project that no longer exists. A dropped index is rebuilt on next use.
pub(super) fn prune_bm25_caches() -> PruneResult {
    let mut result = PruneResult {
        scanned: 0,
        removed: 0,
        bytes_freed: 0,
    };
    let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() else {
        return result;
    };
    let Ok(entries) = std::fs::read_dir(data_dir.join("vectors")) else {
        return result;
    };
    let max_bytes = crate::core::config::DEFAULT_BM25_PERSIST_MB * 1024 * 1024;

    for dir in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        result.scanned += 1;

        let quarantined = dir.join("bm25_index.bin.zst.quarantined");
        if let Ok(meta) = std::fs::metadata(&quarantined)
            && std::fs::remove_file(&quarantined).is_ok()
        {
            result.bytes_freed += meta.len();
            result.removed += 1;
        }

        let index_path = dir.join("bm25_index.bin.zst");
        if let Ok(meta) = std::fs::metadata(&index_path)
            && meta.len() > max_bytes
            && std::fs::remove_file(&index_path).is_ok()
        {
            result.bytes_freed += meta.len();
            result.removed += 1;
        }

        if let Ok(root) = std::fs::read_to_string(dir.join("project_root.txt"))
            && !std::path::Path::new(root.trim()).exists()
        {
            remove_dir(&dir, &mut result);
        }
    }
    result
}

/// Drop property graphs whose project no longer exists (the meta file records
/// the absolute root) and pathologically large ones (> 100 MB). A dropped
/// graph is rebuilt on the next query.
pub(super) fn prune_graph_caches() -> PruneResult {
    let mut result = PruneResult {
        scanned: 0,
        removed: 0,
        bytes_freed: 0,
    };
    let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() else {
        return result;
    };
    let Ok(entries) = std::fs::read_dir(data_dir.join("graphs")) else {
        return result;
    };

    for dir in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let meta_file = dir.join("graph.meta.json");
        let db_file = dir.join("graph.db");
        if !meta_file.exists() && !db_file.exists() {
            continue;
        }
        result.scanned += 1;

        let orphaned = std::fs::read_to_string(&meta_file)
            .ok()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|v| v.get("project_root")?.as_str().map(String::from))
            .is_some_and(|root| !root.is_empty() && !std::path::Path::new(&root).exists());
        let oversized = std::fs::metadata(&db_file).is_ok_and(|m| m.len() > 100 * 1024 * 1024);
        if orphaned || oversized {
            remove_dir(&dir, &mut result);
        }
    }
    result
}

fn remove_dir(dir: &std::path::Path, result: &mut PruneResult) {
    let freed = dir_size(dir);
    if std::fs::remove_dir_all(dir).is_ok() {
        result.bytes_freed += freed;
        result.removed += 1;
    }
}

fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| e.path())
        .map(|p| {
            if p.is_dir() {
                dir_size(&p)
            } else {
                std::fs::metadata(&p).map_or(0, |m| m.len())
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{orphaned_knowledge_step, prune_bm25_caches, prune_graph_caches};

    /// A knowledge store whose project is not on disk is reported and kept.
    #[test]
    fn knowledge_of_a_missing_project_is_kept() {
        let data = crate::core::data_dir::isolated_data_dir();
        let store = data.path().join("knowledge/abc");
        std::fs::create_dir_all(&store).unwrap();
        let knowledge = crate::core::knowledge::ProjectKnowledge::new("/nonexistent/sobre-moved");
        std::fs::write(
            store.join("knowledge.json"),
            serde_json::to_string(&knowledge).unwrap(),
        )
        .unwrap();

        let step = orphaned_knowledge_step();
        assert!(store.join("knowledge.json").exists());
        assert_eq!(step.warnings.len(), 1, "{:?}", step.warnings);
        assert!(step.warnings[0].starts_with("kept knowledge store"));
    }

    #[test]
    fn prune_drops_indexes_of_deleted_projects_only() {
        let data = crate::core::data_dir::isolated_data_dir();
        let live = tempfile::tempdir().unwrap();
        let live_root = live.path().to_string_lossy().to_string();
        let gone_root = "/nonexistent/sobre-prune-test".to_string();

        let vectors = data.path().join("vectors");
        let graphs = data.path().join("graphs");
        for (name, root) in [("gone", &gone_root), ("live", &live_root)] {
            let bm25 = vectors.join(name);
            std::fs::create_dir_all(&bm25).unwrap();
            std::fs::write(bm25.join("project_root.txt"), root).unwrap();
            std::fs::write(bm25.join("bm25_index.bin.zst"), b"x").unwrap();

            let graph = graphs.join(name);
            std::fs::create_dir_all(&graph).unwrap();
            std::fs::write(graph.join("graph.db"), b"x").unwrap();
            std::fs::write(
                graph.join("graph.meta.json"),
                serde_json::json!({ "project_root": root }).to_string(),
            )
            .unwrap();
        }

        let bm25 = prune_bm25_caches();
        assert_eq!((bm25.scanned, bm25.removed), (2, 1));
        assert!(!vectors.join("gone").exists());
        assert!(vectors.join("live").exists());

        let graph = prune_graph_caches();
        assert_eq!((graph.scanned, graph.removed), (2, 1));
        assert!(!graphs.join("gone").exists());
        assert!(graphs.join("live").exists());
    }
}
