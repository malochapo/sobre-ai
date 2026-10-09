use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::server::tool_trait::{
    McpTool, ShellOutcome, ToolContext, ToolOutput, get_bool, get_int, get_str,
};
use crate::tool_defs::tool_def;

pub struct SobreShellTool;

impl McpTool for SobreShellTool {
    fn name(&self) -> &'static str {
        "sobre_shell"
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_shell",
            "WORKFLOW: preferred — auto-compresses output (build/test/log).\n\
             raw=true for verbatim output.\n\
             [exit:N] on errors (lossless).\n\
             Runs every command as given; the agent host decides what may run.\n\
             Sets GIT_TERMINAL_PROMPT=0, GIT_PAGER=cat, PAGER=cat, a UTF-8 locale if none, stdin=/dev/null.\n\
             Multi-line scripts: write them to a file, then run the file.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command" },
                    "raw": { "type": "boolean", "description": "Skip compression (verbatim)" },
                    "cwd": { "type": "string", "description": "Working dir (persists across calls)" },
                    "timeout_ms": { "type": "integer", "description": "Job lifetime in ms (max 3600000) — NOT the inline wait. A command still running at the ~110s foreground cap detaches to a pollable shell_* job and keeps running up to timeout_ms." },
                    "env": { "type": "object", "description": "Extra env vars", "additionalProperties": { "type": "string" } },
                    "run_in_background": { "type": "boolean", "description": "Detach immediately and return a job id. The command keeps timeout_ms; poll or cancel with background_action and job_id." },
                    "background_action": { "type": "string", "enum": ["status", "cancel"], "description": "Inspect or cancel a background sobre_shell job." },
                    "job_id": { "type": "string", "description": "Job id returned by run_in_background." }
                },
                "anyOf": [
                    { "required": ["command"] },
                    { "required": ["background_action", "job_id"] }
                ]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        if let Some(action) = get_str(args, "background_action") {
            let id = get_str(args, "job_id").ok_or_else(|| {
                ErrorData::invalid_params("job_id is required with background_action", None)
            })?;
            let state = match action.as_str() {
                "status" => crate::server::background_shell::status(&id),
                "cancel" => crate::server::background_shell::cancel(&id),
                _ => {
                    return Err(ErrorData::invalid_params(
                        "background_action must be status or cancel",
                        None,
                    ));
                }
            };
            let Some(state) = state else {
                return Ok(ToolOutput {
                    shell_outcome: Some(ShellOutcome::Exit(1)),
                    content_blocks: None,
                    ..ToolOutput::simple(format!("[background:{id} not found]"))
                });
            };
            let (text, exit_code) = match state {
                crate::server::background_shell::JobState::Running if action == "cancel" => (
                    format!(
                        "[background:{id} cancel requested; still stopping — check with background_action=\"status\"]"
                    ),
                    0,
                ),
                crate::server::background_shell::JobState::Running => {
                    (format!("[background:{id} running]"), 0)
                }
                crate::server::background_shell::JobState::Completed { output, exit_code } => (
                    format!(
                        "[background:{id} completed]\n{}{}",
                        redact_shell_output_secrets(&output),
                        if exit_code == 0 {
                            String::new()
                        } else {
                            format!("\n[exit:{exit_code}]")
                        }
                    ),
                    exit_code,
                ),
                crate::server::background_shell::JobState::Cancelled { output } => (
                    format!(
                        "[background:{id} cancelled]\n{}\n[exit:130]",
                        redact_shell_output_secrets(&output)
                    ),
                    130,
                ),
            };
            return Ok(ToolOutput {
                shell_outcome: Some(ShellOutcome::Exit(exit_code)),
                content_blocks: None,
                ..ToolOutput::simple(text)
            });
        }
        let command = get_str(args, "command")
            .ok_or_else(|| ErrorData::invalid_params("command is required", None))?;
        let timeout_ms = get_int(args, "timeout_ms").and_then(|n| u64::try_from(n).ok());

        tokio::task::block_in_place(|| {
            let session_lock = ctx
                .session
                .as_ref()
                .ok_or_else(|| ErrorData::internal_error("session not available", None))?;

            let explicit_cwd = get_str(args, "cwd");
            let had_explicit_cwd = explicit_cwd.is_some();
            let effective_cwd = {
                let guard = crate::server::bounded_lock::read(session_lock, "sobre_shell_cwd");
                match guard {
                    Some(session) => session.effective_cwd(explicit_cwd.as_deref()),
                    None => explicit_cwd.unwrap_or_else(|| ".".to_string()),
                }
            };
            let extra_env = extra_env_from_args(args);

            {
                let Some(mut session) =
                    crate::server::bounded_lock::write(session_lock, "sobre_shell_write")
                else {
                    tracing::debug!(
                        "[sobre_shell: session lock timeout, proceeding without update]"
                    );
                    let cmd_clone = command.clone();
                    let cwd_clone = effective_cwd.clone();
                    let (raw_output, exit_code) = crate::server::execute::execute_command_with_env(
                        &cmd_clone, &cwd_clone, &extra_env, timeout_ms,
                    );
                    let output = redact_shell_output_secrets(&raw_output);
                    // Keep failure reporting consistent on this degraded path:
                    // same [exit:N] footer and the same structured outcome.
                    let exit_suffix = match exit_code {
                        0 => String::new(),
                        124 => "\n[exit:124 — command timed out]".to_string(),
                        _ => format!("\n[exit:{exit_code}]"),
                    };
                    return Ok(ToolOutput {
                        shell_outcome: Some(ShellOutcome::Exit(exit_code)),
                        content_blocks: None,
                        ..ToolOutput::simple(format!("{output}{exit_suffix}"))
                    });
                };
                // An explicit `cwd` param is the client
                // telling us where it now works (worktree switches arrive
                // this way, not as `cd` commands) — persist it so path
                // resolution's divergence check tracks the live checkout.
                if had_explicit_cwd {
                    session.note_explicit_cwd(&effective_cwd);
                }
                session.update_shell_cwd(&command);
                let root_missing = session
                    .project_root
                    .as_deref()
                    .is_none_or(|r| r.trim().is_empty());
                if root_missing {
                    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string());
                    if let Some(root) = crate::core::protocol::detect_project_root(&effective_cwd)
                        && home.as_deref() != Some(root.as_str())
                    {
                        session.project_root = Some(root);
                    }
                }
            }

            let raw = get_bool(args, "raw").unwrap_or(false);

            let crp_mode = ctx.crp_mode;
            let cmd_clone = command.clone();
            let cwd_clone = effective_cwd;
            let proactive_block = if raw {
                None
            } else {
                crate::core::relevance_tracker::proactive_context(&format!(
                    "sobre_shell command={cmd_clone} cwd={cwd_clone}"
                ))
            };

            let auto_background = should_auto_background(&cmd_clone, timeout_ms);
            if get_bool(args, "run_in_background").unwrap_or(false) || auto_background {
                let job_id = crate::server::background_shell::start(
                    cmd_clone, cwd_clone, extra_env, timeout_ms,
                );
                let mode = if auto_background {
                    "auto-background"
                } else {
                    "background"
                };
                return Ok(ToolOutput {
                    shell_outcome: Some(ShellOutcome::Exit(0)),
                    content_blocks: None,
                    ..ToolOutput::simple(format!(
                        "[{mode}:{job_id} started — use sobre_shell(background_action=\"status\", job_id=\"{job_id}\") to poll or background_action=\"cancel\" to stop it]"
                    ))
                });
            }

            // Foreground runs still detach onto a pollable job if they outlast
            // the soft cap, so the MCP host's ~120s abort never strands the
            // result behind an unresolvable task id.
            //
            // `timeout_ms` is the *job's* lifetime, never the foreground
            // wait, so it must not raise this cap. Raising it bought no extra
            // inline wait — the host aborts at ~120s regardless — it only
            // suppressed our own detach, producing exactly the unresolvable
            // task id the cap exists to prevent. Separate knobs, one direction:
            let soft_cap = std::time::Duration::from_millis(foreground_soft_cap_ms());
            let progress_sender = ctx.progress_sender.clone();
            let progress_label: String = cmd_clone.chars().take(60).collect();
            let cap_secs = soft_cap.as_secs_f64();
            let on_tick = |elapsed: std::time::Duration| {
                if let Some(ref sender) = progress_sender {
                    sender.send(
                        elapsed.as_secs_f64(),
                        Some(cap_secs),
                        Some(format!(
                            "sobre_shell: {}s elapsed — {progress_label}",
                            elapsed.as_secs()
                        )),
                    );
                }
            };
            let (raw_output, exit_code) =
                match crate::server::background_shell::run_foreground_or_detach(
                    cmd_clone.clone(),
                    cwd_clone.clone(),
                    extra_env.clone(),
                    timeout_ms,
                    soft_cap,
                    Some(&on_tick),
                ) {
                    crate::server::background_shell::ForegroundResult::Finished {
                        output,
                        exit_code,
                    } => (output, exit_code),
                    crate::server::background_shell::ForegroundResult::Detached { job_id } => {
                        return Ok(ToolOutput {
                            shell_outcome: Some(ShellOutcome::Exit(0)),
                            content_blocks: None,
                            // A detach is not a failure — the command is
                            // alive and its output is recoverable, so say so
                            // rather than leaving the caller to infer a hang.
                            ..ToolOutput::simple(format!(
                                "[auto-background:{job_id} still running — passed the {}s foreground cap, not an error; output is kept and delivered by sobre_shell(background_action=\"status\", job_id=\"{job_id}\"), or background_action=\"cancel\" to stop it]",
                                soft_cap.as_secs()
                            ))
                        });
                    }
                };

            // Structured diagnostics — same hook as the CLI path.
            crate::core::diagnostics_store::record_from_shell(&cmd_clone, &raw_output, exit_code);

            let output = redact_shell_output_secrets(&raw_output);

            let (result_out, original, saved, tee_hint) = if raw {
                let tokens = crate::core::tokens::count_tokens(&output);
                (output, tokens, 0, String::new())
            } else {
                let result =
                    crate::tools::sobre_shell::handle(&cmd_clone, &output, exit_code, crp_mode);
                let original = crate::core::tokens::count_tokens(&output);
                let sent = crate::core::tokens::count_tokens(&result);
                let saved = original.saturating_sub(sent);

                // Same rule as the CLI path: whatever compression cut is saved,
                // and `sobre_expand` is the one way back to it.
                let tee_hint = if crate::shell::tee_policy::should_tee(
                    &output,
                    &result,
                    is_timeout_notice_only(&output, exit_code),
                ) {
                    crate::shell::save_tee(&cmd_clone, &output)
                        .and_then(|p| {
                            let id = std::path::Path::new(&p).file_name()?.to_str()?.to_string();
                            Some(format!(
                                "\n[sobre: full output at {p}, or sobre_expand(id=\"{id}\")]"
                            ))
                        })
                        .unwrap_or_default()
                } else {
                    String::new()
                };

                (result, original, saved, tee_hint)
            };

            let shell_mismatch = if cfg!(windows) && !raw {
                shell_mismatch_hint(&command, &result_out)
            } else {
                String::new()
            };

            let result_out = crate::core::redaction::redact_text(&result_out);
            // Exit 124 = timeout signal (from timeout(1) / sobre
            // shell timeout). Make it explicit so agents don't confuse a
            // timed-out command with a successful empty result.
            let exit_suffix = match exit_code {
                0 => String::new(),
                124 => "\n[exit:124 — command timed out]".to_string(),
                _ => format!("\n[exit:{exit_code}]"),
            };
            let nudge = if raw { "" } else { search_tool_nudge(&command) };
            let final_out = format!("{result_out}{tee_hint}{shell_mismatch}{nudge}{exit_suffix}");
            let final_out = if let Some(block) = proactive_block {
                format!("{final_out}{block}")
            } else {
                final_out
            };

            Ok(ToolOutput {
                text: final_out,
                original_tokens: original,
                saved_tokens: saved,
                shell_outcome: Some(ShellOutcome::Exit(exit_code)),
                content_blocks: None,
            })
        })
    }
}

/// A timeout notice is framework metadata, not recoverable command output. Do
/// not archive it as a tee artifact: expanding it cannot recover any bytes.
///
/// Keyed on what precedes the marker rather than on the notice's exact shape,
/// so enriching it (the idle-timeout wording, the still-running segment list)
/// cannot silently turn every timeout back into an archived artifact.
fn is_timeout_notice_only(output: &str, exit_code: i32) -> bool {
    exit_code == 124
        && crate::server::execute::output_before_timeout_marker(output).is_some_and(str::is_empty)
}

fn search_tool_nudge(command: &str) -> &'static str {
    let cmd = command.trim();
    let first_word = cmd.split_whitespace().next().unwrap_or("");
    if !cmd.contains('|') {
        match first_word {
            "grep" | "rg" | "egrep" | "fgrep" | "ag" => {
                return "\n[hint: use sobre_search for structured, cached results with symbol/semantic modes]";
            }
            "find" => {
                return "\n[hint: use sobre_glob or sobre_tree for structured file discovery]";
            }
            "ls" | "exa" | "eza" => {
                return "\n[hint: use sobre_tree for structured directory listing]";
            }
            _ => {}
        }
    }
    ""
}

fn shell_mismatch_hint(command: &str, output: &str) -> String {
    let shell = crate::shell::shell_name();
    let is_posix = matches!(shell.as_str(), "bash" | "sh" | "zsh" | "fish");
    let has_error = output.contains("is not recognized")
        || output.contains("not found")
        || output.contains("command not found");

    if !has_error {
        return String::new();
    }

    let powershell_cmds = [
        "Get-Content",
        "Select-Object",
        "Get-ChildItem",
        "Set-Location",
        "Where-Object",
        "ForEach-Object",
        "Select-String",
        "Invoke-Expression",
        "Write-Output",
    ];
    let uses_powershell = powershell_cmds
        .iter()
        .any(|c| command.contains(c) || command.contains(&c.to_lowercase()));

    if is_posix && uses_powershell {
        format!(
            "\n[shell: {shell} — use POSIX commands (cat, head, grep, find, ls) not PowerShell cmdlets]"
        )
    } else {
        String::new()
    }
}

/// The `env` argument as a map. Every variable is set as given: what a
/// command may do is the agent host's decision, not sobre's (invariant 5).
fn extra_env_from_args(args: &Map<String, Value>) -> std::collections::HashMap<String, String> {
    args.get("env")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|val| (k.clone(), val.to_string())))
                .collect()
        })
        .unwrap_or_default()
}
fn redact_shell_output_secrets(output: &str) -> String {
    let (redacted, matches) = crate::core::secret_detection::scan_and_redact(output);
    if !matches.is_empty() {
        let names: Vec<&str> = matches.iter().map(|m| m.pattern_name).collect();
        tracing::warn!(
            "[SHELL SECRET REDACTION] {} secret(s) redacted from shell output: {}",
            matches.len(),
            names.join(", ")
        );
    }
    redacted
}

/// The Codex MCP client abandons a tool call after five minutes. A Cargo test
/// with an explicit five-minute-or-longer shell timeout cannot reliably finish
/// inside that transport deadline (and can wait on Cargo's target lock), so
/// detach it and return a pollable job instead.
fn should_auto_background(command: &str, timeout_ms: Option<u64>) -> bool {
    timeout_ms.is_some_and(|timeout| timeout >= 300_000)
        && command
            .lines()
            .any(|line| line.trim_start().starts_with("cargo test"))
}

/// Foreground wait budget before a still-running command is detached into a
/// pollable background job. Kept below the MCP host's ~120s tool-call abort so
/// the caller always receives a real `shell_*` job id instead of an
/// unresolvable task id.
fn foreground_soft_cap_ms() -> u64 {
    110_000
}

#[cfg(test)]
mod tests {
    use super::{extra_env_from_args, is_timeout_notice_only, should_auto_background};

    #[test]
    fn env_vars_are_all_set_as_given() {
        let args = serde_json::json!({"env": {"PATH": "/opt/bin", "RUST_LOG": "debug"}});
        let env = extra_env_from_args(args.as_object().unwrap());
        assert_eq!(env.get("PATH").map(String::as_str), Some("/opt/bin"));
        assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("debug"));
    }

    #[test]
    fn long_cargo_test_is_auto_backgrounded() {
        assert!(should_auto_background(
            "cargo test --lib a\ncargo test --lib b",
            Some(3_600_000)
        ));
        assert!(should_auto_background("cargo test --lib a", Some(300_000)));
        assert!(!should_auto_background("cargo test --lib a", Some(299_999)));
    }

    #[test]
    fn timeout_notice_without_child_output_is_not_recoverable() {
        assert!(is_timeout_notice_only(
            "ERROR: command timed out after 200ms",
            124
        ));
        assert!(is_timeout_notice_only(
            "  ERROR: command timed out after 200ms\n",
            124
        ));
        assert!(!is_timeout_notice_only(
            "useful output\nERROR: command timed out after 200ms",
            124
        ));
        assert!(!is_timeout_notice_only(
            "ERROR: command timed out after 200ms",
            1
        ));
        // The notice now carries the idle wording and the still-running
        // segment list. It is still pure metadata — nothing to recover — so it
        // must not become a tee artifact just because it grew.
        assert!(is_timeout_notice_only(
            "ERROR: command timed out after 200ms without new output\n\
             [still running at timeout: sleep 300]",
            124
        ));
        assert!(!is_timeout_notice_only(
            "useful output\nERROR: command timed out after 200ms\n\
             [still running at timeout: sleep 300]",
            124
        ));
        // Exit 124 from something that is not our watchdog carries no marker.
        assert!(!is_timeout_notice_only("some tool output", 124));
    }
}
