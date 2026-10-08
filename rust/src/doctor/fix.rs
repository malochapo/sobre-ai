use chrono::Utc;

use super::{DIM, RST, compact_score, display_user_path, print_compact_status};

pub(super) struct DoctorFixOptions {
    pub json: bool,
}

pub(super) fn run_fix(opts: &DoctorFixOptions) -> Result<i32, String> {
    let report = build_and_persist_fix_report(opts.json)?;
    if opts.json {
        let json_text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        println!("{json_text}");
    } else {
        let (passed, total) = compact_score();
        print_compact_status(passed, total);
        if let Ok(path) = crate::core::setup_report::doctor_report_path() {
            println!("  {DIM}report saved:{RST} {}", display_user_path(&path));
        }
    }
    Ok(i32::from(!report.success))
}

fn build_and_persist_fix_report(
    quiet: bool,
) -> Result<crate::core::setup_report::SetupReport, String> {
    use crate::core::setup_report::{
        PlatformInfo, SetupItem, SetupReport, SetupStepReport, doctor_report_path,
    };

    let _quiet_guard = quiet.then(crate::core::runtime_flags::scoped_quiet);
    let started_at = Utc::now();
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;

    let mut steps: Vec<SetupStepReport> = Vec::new();

    let mut mcp_step = SetupStepReport {
        name: "mcp_config".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let binary = crate::core::portable_binary::resolve_portable_binary();
    let targets = crate::core::editor_registry::build_targets(&home);
    for t in &targets {
        if !t.detect_path.exists() {
            continue;
        }
        let short = t.config_path.to_string_lossy().to_string();

        let res = crate::core::editor_registry::write_config_with_options(
            t,
            &binary,
            crate::core::editor_registry::WriteOptions {
                overwrite_invalid: true,
            },
        );

        match res {
            Ok(r) => {
                let status = match r.action {
                    crate::core::editor_registry::WriteAction::Created => "created",
                    crate::core::editor_registry::WriteAction::Updated => "updated",
                    crate::core::editor_registry::WriteAction::Already => "already",
                };
                mcp_step.items.push(SetupItem {
                    name: t.name.to_string(),
                    status: status.to_string(),
                    path: Some(short),
                    note: r.note,
                });
            }
            Err(e) => {
                mcp_step.ok = false;
                mcp_step.items.push(SetupItem {
                    name: t.name.to_string(),
                    status: "error".to_string(),
                    path: Some(short),
                    note: Some(e.clone()),
                });
                mcp_step.errors.push(format!("{}: {e}", t.name));
            }
        }
    }
    if mcp_step.items.is_empty() {
        mcp_step
            .warnings
            .push("no supported AI tools detected; skipped MCP config repair".to_string());
    }
    steps.push(mcp_step);

    // Resolve workspace/user dual-scope conflicts (issue #338)
    let mut ws_scope_step = SetupStepReport {
        name: "workspace_scope".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let user_scope_mcp_locations = super::sobre_mcp_location_names(&home);
    let ws_fixed = super::workspace_scope::fix_workspace_dual_scope(&user_scope_mcp_locations);
    ws_scope_step.items.push(SetupItem {
        name: "dual_scope_dedup".to_string(),
        status: if ws_fixed > 0 {
            format!("fixed {ws_fixed}")
        } else {
            "clean".to_string()
        },
        path: None,
        note: if ws_fixed > 0 {
            Some("removed sobre from workspace-scope (user-scope preferred)".to_string())
        } else {
            None
        },
    });
    steps.push(ws_scope_step);

    let mut hooks_step = SetupStepReport {
        name: "agent_hooks".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let targets = crate::core::editor_registry::build_targets(&home);
    for t in &targets {
        if !t.detect_path.exists() || t.agent_key.trim().is_empty() {
            continue;
        }
        crate::hooks::install_agent_hook(&t.agent_key);
        hooks_step.items.push(SetupItem {
            name: format!("{} hooks", t.name),
            status: "installed".to_string(),
            path: Some(t.detect_path.to_string_lossy().to_string()),
            note: Some("merge-based install/repair".to_string()),
        });
    }
    if !hooks_step.items.is_empty() {
        steps.push(hooks_step);
    }

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
        note: Some(
            "cleared MCP startup history (fixes backoff after IDE restart loops)".to_string(),
        ),
    });
    steps.push(startup_step);

    // Prune knowledge stores whose project_root was deleted (removed git
    // worktrees, thrown-away projects). They can never be written again, so
    // their per-store eviction cap can never self-heal — pure accumulated bloat
    // (GH #615). Only the explicit --fix path deletes; the background lifecycle
    // never does, since a missing root can also be a temporarily-unmounted drive.
    let mut orphan_step = SetupStepReport {
        name: "orphaned_knowledge".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let prune = crate::core::knowledge::maintenance::prune_orphaned_stores();
    orphan_step.items.push(SetupItem {
        name: "prune".to_string(),
        status: if prune.removed > 0 {
            format!("removed {}", prune.removed)
        } else {
            "clean".to_string()
        },
        path: None,
        note: Some(if prune.removed > 0 {
            format!(
                "pruned {} knowledge store(s) for deleted projects ({} reclaimed)",
                prune.removed,
                super::common::human_bytes(prune.reclaimed_bytes)
            )
        } else {
            "no knowledge stores for deleted projects".to_string()
        }),
    });
    steps.push(orphan_step);

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
        platform: PlatformInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        },
        steps,
        warnings: Vec::new(),
        errors: Vec::new(),
    };

    let path = doctor_report_path()?;
    let json_text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    crate::config_io::write_atomic_with_backup(&path, &json_text)?;

    Ok(report)
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
    use super::{prune_bm25_caches, prune_graph_caches};

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
