use chrono::Utc;

use super::{
    DIM, RST, compact_score, display_user_path, print_compact_status, shell_aliases_outcome,
};

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

    let mut shell_step = SetupStepReport {
        name: "shell_hook".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let before = shell_aliases_outcome();
    if before.ok {
        shell_step.items.push(SetupItem {
            name: "init --global".to_string(),
            status: "already".to_string(),
            path: None,
            note: None,
        });
    } else {
        if quiet {
            crate::cli::install_shell_hook_quiet(&["--global".to_string()]);
        } else {
            crate::cli::install_shell_hook(&["--global".to_string()]);
        }
        let after = shell_aliases_outcome();
        shell_step.ok = after.ok;
        shell_step.items.push(SetupItem {
            name: "init --global".to_string(),
            status: if after.ok {
                "fixed".to_string()
            } else {
                "failed".to_string()
            },
            path: None,
            note: if after.ok {
                None
            } else {
                Some("shell hook still not detected by doctor checks".to_string())
            },
        });
        if !after.ok {
            shell_step
                .warnings
                .push("shell hook not detected after init --global".to_string());
        }
    }
    steps.push(shell_step);

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
    let user_scope_mcp_locations = super::lean_ctx_mcp_location_names(&home);
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
            Some("removed lean-ctx from workspace-scope (user-scope preferred)".to_string())
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
        crate::hooks::install_agent_hook(&t.agent_key, true);
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

    let mut bm25_step = SetupStepReport {
        name: "bm25_cache_prune".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let prune_result = crate::cli::prune_bm25_caches();
    bm25_step.items.push(SetupItem {
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
    });
    steps.push(bm25_step);

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
