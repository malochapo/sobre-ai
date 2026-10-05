use std::path::PathBuf;

use crate::core::editor_registry::{ConfigType, EditorTarget, WriteAction, WriteOptions};
use crate::core::portable_binary::resolve_portable_binary;
use crate::core::setup_report::{PlatformInfo, SetupItem, SetupReport, SetupStepReport};
use crate::hooks::{HookMode, recommend_hook_mode};
use chrono::Utc;
mod mcp;
pub use mcp::*;
mod helpers;
pub(crate) use helpers::*;

pub fn claude_config_dir(home: &std::path::Path) -> PathBuf {
    crate::core::editor_registry::claude_state_dir(home)
}

#[cfg(test)]
pub(crate) struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl EnvVarGuard {
    pub(crate) fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: EnvVarGuard is test-only; env-mutating tests are serialized,
        // so no other thread reads or writes the environment concurrently.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

#[cfg(test)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            // SAFETY: EnvVarGuard is test-only; env-mutating tests are serialized,
            // so no other thread reads or writes the environment concurrently.
            unsafe { std::env::set_var(self.key, previous) };
        } else {
            // SAFETY: EnvVarGuard is test-only; env-mutating tests are serialized,
            // so no other thread reads or writes the environment concurrently.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct SetupOptions {
    pub non_interactive: bool,
    pub yes: bool,
    pub fix: bool,
    pub json: bool,
    pub no_auto_approve: bool,
    pub skip_proxy: bool,
    pub skip_rules: bool,
    /// Leave the shell rc untouched (`init --no-shell-hook`).
    pub skip_shell_hook: bool,
    /// `init --agent-aliases` / `--no-agent-aliases`; `None` keeps what is
    /// installed.
    pub agent_aliases: Option<bool>,
}

pub fn run_setup_with_options(opts: SetupOptions) -> Result<SetupReport, String> {
    let _quiet_guard = opts.json.then(crate::core::runtime_flags::scoped_quiet);
    let started_at = Utc::now();
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    let binary = resolve_portable_binary();
    let home_str = home.to_string_lossy().to_string();

    // Commit to the XDG layout (and drain any residual ~/.lean-ctx) so a stray
    // marker can never re-collapse config/data/state/cache later (GL #623).
    crate::core::layout_pin::heal();

    let mut steps: Vec<SetupStepReport> = Vec::new();

    // Step: Shell Hook
    let mut shell_step = SetupStepReport {
        name: "shell_hook".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    if (!opts.non_interactive || opts.yes) && !opts.skip_shell_hook {
        if opts.json {
            crate::cli::install_shell_hook_quiet(&["--global".to_string()]);
        } else {
            crate::cli::install_shell_hook(&["--global".to_string()]);
        }
        crate::shell_hook::install_all(opts.json, opts.agent_aliases);
        #[cfg(not(windows))]
        {
            let hook_content = crate::cli::generate_hook_posix(&binary);
            if crate::shell::is_container() {
                crate::cli::write_env_sh_for_containers(&hook_content);
                shell_step.items.push(SetupItem {
                    name: "env_sh".to_string(),
                    status: "created".to_string(),
                    path: Some(crate::core::paths::config_dir().map_or_else(
                        |_| "~/.config/lean-ctx/env.sh".to_string(),
                        |d| d.join("env.sh").to_string_lossy().to_string(),
                    )),
                    note: Some("Docker/CI helper (BASH_ENV / CLAUDE_ENV_FILE)".to_string()),
                });
            } else {
                shell_step.items.push(SetupItem {
                    name: "env_sh".to_string(),
                    status: "skipped".to_string(),
                    path: None,
                    note: Some("not a container environment".to_string()),
                });
            }
        }
        shell_step.items.push(SetupItem {
            name: "init --global".to_string(),
            status: "ran".to_string(),
            path: None,
            note: None,
        });
        shell_step.items.push(SetupItem {
            name: "universal_shell_hook".to_string(),
            status: "installed".to_string(),
            path: None,
            note: Some("~/.zshenv, ~/.bashenv, agent aliases".to_string()),
        });
    } else {
        shell_step
            .warnings
            .push("non_interactive_without_yes: shell hook not installed (use --yes)".to_string());
        shell_step.ok = false;
        shell_step.items.push(SetupItem {
            name: "init --global".to_string(),
            status: "skipped".to_string(),
            path: None,
            note: Some("requires --yes in --non-interactive mode".to_string()),
        });
    }
    steps.push(shell_step);

    // Step: Editor MCP config
    let mut editor_step = SetupStepReport {
        name: "editors".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };

    let targets = crate::core::editor_registry::build_targets(&home);
    for target in &targets {
        let short_path = shorten_path(&target.config_path.to_string_lossy(), &home_str);
        if !target.detect_path.exists() {
            editor_step.items.push(SetupItem {
                name: target.name.to_string(),
                status: "not_detected".to_string(),
                path: Some(short_path),
                note: None,
            });
            continue;
        }

        let mode = if target.agent_key.is_empty() {
            HookMode::Mcp
        } else {
            recommend_hook_mode(&target.agent_key)
        };

        let res = crate::core::editor_registry::write_config_with_options(
            target,
            &binary,
            WriteOptions {
                overwrite_invalid: opts.fix,
            },
        );
        match res {
            Ok(w) => {
                let note_parts: Vec<String> = [Some(format!("mode={mode}")), w.note]
                    .into_iter()
                    .flatten()
                    .collect();
                editor_step.items.push(SetupItem {
                    name: target.name.to_string(),
                    status: match w.action {
                        WriteAction::Created => "created".to_string(),
                        WriteAction::Updated => "updated".to_string(),
                        WriteAction::Already => "already".to_string(),
                    },
                    path: Some(short_path),
                    note: Some(note_parts.join("; ")),
                });
            }
            Err(e) => {
                editor_step.ok = false;
                editor_step.items.push(SetupItem {
                    name: target.name.to_string(),
                    status: "error".to_string(),
                    path: Some(short_path),
                    note: Some(e),
                });
            }
        }
    }
    steps.push(editor_step);

    // Step: Agent rules — always, unless `--skip-rules`
    let mut rules_step = SetupStepReport {
        name: "agent_rules".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let should_inject = !opts.skip_rules;

    if should_inject {
        let rules_result = crate::rules_inject::inject_all_rules(&home);
        for n in rules_result.injected {
            rules_step.items.push(SetupItem {
                name: n,
                status: "injected".to_string(),
                path: None,
                note: None,
            });
        }
        for n in rules_result.updated {
            rules_step.items.push(SetupItem {
                name: n,
                status: "updated".to_string(),
                path: None,
                note: None,
            });
        }
        for n in rules_result.already {
            rules_step.items.push(SetupItem {
                name: n,
                status: "already".to_string(),
                path: None,
                note: None,
            });
        }
        if !rules_result.backed_up.is_empty() {
            for bak in &rules_result.backed_up {
                rules_step.items.push(SetupItem {
                    name: "backup".to_string(),
                    status: "created".to_string(),
                    path: Some(bak.clone()),
                    note: Some("previous version backed up".to_string()),
                });
            }
        }
        for e in rules_result.errors {
            rules_step.ok = false;
            rules_step.errors.push(e);
        }
    } else {
        rules_step.items.push(SetupItem {
            name: "agent_rules".to_string(),
            status: "skipped".to_string(),
            path: None,
            note: Some("--skip-rules flag set".to_string()),
        });
    }
    steps.push(rules_step);

    // Step: rules dedup (#578) — auto-heal duplicated lean-ctx guidance so a
    // client never pays for the rules block twice per session. Only
    // lean-ctx-owned files / marked blocks are touched (backups for edits);
    // user-maintained content is left alone by `auto_apply`.
    if should_inject {
        let mut dedup_step = SetupStepReport {
            name: "rules_dedup".to_string(),
            ok: true,
            items: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        };
        let project = std::env::current_dir().unwrap_or_else(|_| home.clone());
        for line in crate::cli::rules_dedup::auto_apply(&home, &project) {
            let failed = line.starts_with("FAILED");
            if failed {
                dedup_step.warnings.push(line.clone());
            }
            dedup_step.items.push(SetupItem {
                name: "dedup".to_string(),
                status: if failed { "failed" } else { "applied" }.to_string(),
                path: None,
                note: Some(line),
            });
        }
        steps.push(dedup_step);
    }

    // Step: Agent-specific hooks (all detected agents)
    let mut hooks_step = SetupStepReport {
        name: "agent_hooks".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    for target in &targets {
        if !target.detect_path.exists() || target.agent_key.is_empty() {
            continue;
        }
        let mode = recommend_hook_mode(&target.agent_key);
        crate::hooks::install_agent_hook_with_mode(&target.agent_key, true, mode);
        let mcp_note = match configure_agent_mcp(&target.agent_key) {
            Ok(()) => "; MCP config updated".to_string(),
            Err(e) => format!("; MCP config skipped: {e}"),
        };
        hooks_step.items.push(SetupItem {
            name: format!("{} hooks", target.name),
            status: "installed".to_string(),
            path: Some(target.detect_path.to_string_lossy().to_string()),
            note: Some(format!(
                "mode={mode}; merge-based install/repair (preserves other hooks/plugins){mcp_note}"
            )),
        });
    }
    if !hooks_step.items.is_empty() {
        steps.push(hooks_step);
    }

    // Step: tools. There is one surface and no key to write — setup reports
    // its size rather than pinning anything.
    let mut tools_step = SetupStepReport {
        name: "tools".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    {
        let advertised = crate::server::registry::build_registry().tool_defs().len();
        tools_step.items.push(SetupItem {
            name: "tools".to_string(),
            status: "ok".to_string(),
            path: None,
            note: Some(format!(
                "{advertised} tools advertised, the same for every client"
            )),
        });
    }
    steps.push(tools_step);

    // Step: Proxy autostart + env vars (respects opt-in)
    let mut proxy_step = SetupStepReport {
        name: "proxy".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    if opts.skip_proxy {
        proxy_step.items.push(SetupItem {
            name: "proxy".to_string(),
            status: "skipped".to_string(),
            path: None,
            note: Some("Proxy not enabled (run `lean-ctx proxy enable`)".to_string()),
        });
    } else {
        let proxy_cfg = crate::core::config::Config::load();
        if proxy_cfg.proxy.enabled == Some(true) {
            let proxy_port = crate::proxy_setup::default_port();
            crate::proxy_autostart::install(proxy_port, true);
            std::thread::sleep(std::time::Duration::from_millis(500));
            crate::proxy_setup::install_proxy_env(&home, proxy_port, opts.json);
            proxy_step.items.push(SetupItem {
                name: "proxy_autostart".to_string(),
                status: "installed".to_string(),
                path: None,
                note: Some("LaunchAgent/systemd auto-start on login".to_string()),
            });
            proxy_step.items.push(SetupItem {
                name: "proxy_env".to_string(),
                status: "configured".to_string(),
                path: None,
                note: Some("ANTHROPIC_BASE_URL, OPENAI_BASE_URL, GEMINI_API_BASE_URL".to_string()),
            });
        } else {
            proxy_step.items.push(SetupItem {
                name: "proxy".to_string(),
                status: "skipped".to_string(),
                path: None,
                note: Some(
                    "Proxy not opted-in (run `lean-ctx proxy enable` to activate)".to_string(),
                ),
            });
        }
    }
    steps.push(proxy_step);

    // Step: Environment / doctor (compact)
    let mut env_step = SetupStepReport {
        name: "doctor_compact".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let (passed, total) = crate::doctor::compact_score();
    env_step.items.push(SetupItem {
        name: "doctor".to_string(),
        status: format!("{passed}/{total}"),
        path: None,
        note: None,
    });
    if passed != total {
        env_step.warnings.push(format!(
            "doctor compact not fully passing: {passed}/{total}"
        ));
    }
    steps.push(env_step);

    // Project root validation: warn if no root is configured and cwd is broad
    {
        let has_env_root = std::env::var("LEAN_CTX_PROJECT_ROOT").is_ok_and(|v| !v.is_empty());
        if !has_env_root && let Ok(cwd) = std::env::current_dir() {
            let is_home = dirs::home_dir().is_some_and(|h| cwd == h);
            if is_home {
                let mut root_step = SetupStepReport {
                        name: "project_root".to_string(),
                        ok: true,
                        items: Vec::new(),
                        warnings: vec![
                            "No project root set. Running from $HOME can cause excessive scanning. \
                             Start your agent from the project directory or set LEAN_CTX_PROJECT_ROOT.".to_string()
                        ],
                        errors: Vec::new(),
                    };
                root_step.items.push(SetupItem {
                    name: "project_root".to_string(),
                    status: "unconfigured".to_string(),
                    path: None,
                    note: Some("Set LEAN_CTX_PROJECT_ROOT".to_string()),
                });
                steps.push(root_step);
            }
        }
    }

    // Auto-build property graph if inside any recognized project. The marker
    // probe is TCC-guarded (#356): a launchd-standalone setup run never stats
    // markers under ~/Documents — and `may_autoindex_cwd` additionally skips the
    // probe for a non-standalone CLI refresh whose cwd is in a protected dir.
    if let Ok(cwd) = std::env::current_dir()
        && may_autoindex_cwd(&cwd)
        && crate::core::pathutil::has_project_marker(&cwd)
    {
        spawn_index_build_background(&cwd);
    }

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

    let path = SetupReport::default_path()?;
    let mut content =
        serde_json::to_string_pretty(&report).map_err(|e| format!("serialize report: {e}"))?;
    content.push('\n');
    crate::config_io::write_atomic(&path, &content)?;

    Ok(report)
}

/// #356: decide whether a setup refresh may auto-index `cwd`. Returns `false`
/// for any cwd inside a macOS TCC-protected home dir (`~/Documents`, `~/Desktop`,
/// `~/Downloads`) so a `lean-ctx update` run from a project there never stats
/// marker files in it. That stat pops the macOS privacy prompt when lean-ctx is
/// its own TCC responsible process, and a maintenance refresh has no need to
/// trigger it — the graph builds on the next real tool use anyway. On non-macOS
/// hosts `is_under_tcc_protected_dir` is always `false`, so behaviour is
/// unchanged.
fn may_autoindex_cwd(cwd: &std::path::Path) -> bool {
    !crate::core::pathutil::is_under_tcc_protected_dir(cwd)
}

fn spawn_index_build_background(root: &std::path::Path) {
    if matches!(std::env::var("LEAN_CTX_QUIET"), Ok(v) if v.trim() == "1") {
        return;
    }
    let root_str = crate::core::graph_index::normalize_project_root(&root.to_string_lossy());
    if !crate::core::graph_index::is_safe_scan_root_public(&root_str) {
        tracing::info!("[setup: skipping background graph build for unsafe root {root_str}]");
        return;
    }

    let binary = resolve_portable_binary();

    #[cfg(unix)]
    {
        let mut cmd = std::process::Command::new("nice");
        cmd.args(["-n", "19"]);
        if which_ionice_available() {
            cmd.arg("ionice").args(["-c", "3"]);
        }
        cmd.arg(&binary)
            .args(["index", "build", "--root"])
            .arg(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        let _ = cmd.spawn();
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new(&binary)
            .args(["index", "build", "--root"])
            .arg(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
            .spawn();
    }
}

#[cfg(unix)]
fn which_ionice_available() -> bool {
    std::process::Command::new("ionice")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    // #356: a setup refresh (e.g. via `lean-ctx update`) must not auto-index a
    // cwd inside a TCC-protected home dir, or it stats marker files there and
    // pops the macOS privacy prompt. Projects elsewhere index normally.
    #[test]
    fn may_autoindex_cwd_skips_tcc_protected_dirs() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert!(!may_autoindex_cwd(&home.join("Documents/proj")));
        assert!(!may_autoindex_cwd(&home.join("Desktop/proj")));
        assert!(!may_autoindex_cwd(&home.join("Downloads/proj")));
        assert!(may_autoindex_cwd(&home.join("code/proj")));
        assert!(may_autoindex_cwd(std::path::Path::new("/tmp/proj")));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn qoder_agent_targets_include_all_macos_mcp_locations() {
        let home = std::path::Path::new("/Users/tester");
        let targets = agent_mcp_targets("qoder", home).unwrap();
        let paths: Vec<_> = targets.iter().map(|t| t.config_path.as_path()).collect();

        assert_eq!(
            paths,
            vec![
                home.join(".qoder/mcp.json").as_path(),
                home.join("Library/Application Support/Qoder/User/mcp.json")
                    .as_path(),
                home.join("Library/Application Support/Qoder/SharedClientCache/mcp.json")
                    .as_path(),
            ]
        );
        assert!(
            targets
                .iter()
                .all(|t| t.config_type == ConfigType::QoderSettings)
        );
    }
}
