use crate::core::session::SessionState;

#[derive(Clone, Copy, Debug)]
pub struct SessionToolOptions<'a> {
    pub format: Option<&'a str>,
    pub path: Option<&'a str>,
    pub write: bool,
}

/// Save `session` when it holds anything, so replacing it loses nothing.
fn save_if_recorded(session: &mut SessionState) -> Result<(), String> {
    if session.has_record() {
        session.save()
    } else {
        Ok(())
    }
}

pub fn handle(
    session: &mut SessionState,
    action: &str,
    value: Option<&str>,
    session_id: Option<&str>,
    opts: SessionToolOptions<'_>,
) -> String {
    match action {
        // "show" is the natural synonym agents reach for first.
        "status" | "show" => session.format_compact(),

        "load" => {
            // Without an id: this project's latest saved session, never the
            // one of the server's cwd. The live session is saved first.
            let loaded = if let Some(id) = session_id {
                SessionState::load_by_id(id)
            } else {
                session
                    .project_root
                    .as_deref()
                    .and_then(SessionState::load_latest_for_project_root)
                    .filter(|s| s.id != session.id)
            };

            let Some(prev) = loaded else {
                return match session_id {
                    Some(id) => format!("ERROR: no saved session '{id}'"),
                    None => "ERROR: no other saved session for this project".to_string(),
                };
            };
            if let Err(e) = save_if_recorded(session) {
                return format!(
                    "ERROR: the current session could not be saved ({e}); nothing loaded"
                );
            }
            let summary = prev.format_compact();
            session.replace_record(prev);
            format!("Session loaded.\n{summary}")
        }

        "save" => match session.save() {
            Ok(()) => format!("Session {} saved (v{}).", session.id, session.version),
            Err(e) => format!("ERROR: save failed: {e}"),
        },

        "export" => {
            let bundle = crate::core::ccp_session_bundle::build_bundle_v1(session);
            let json = match crate::core::ccp_session_bundle::serialize_bundle_v1_pretty(&bundle) {
                Ok(s) => s,
                Err(e) => return e,
            };

            let format = opts
                .format
                .unwrap_or(if opts.write { "summary" } else { "json" });
            let root = session.project_root.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
            });
            let root_path = std::path::PathBuf::from(&root);

            let mut written: Option<String> = None;
            if opts.write || opts.path.is_some() {
                let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
                let candidate = if let Some(p) = opts.path.or(value) {
                    let p = std::path::PathBuf::from(p);
                    if p.is_absolute() {
                        p
                    } else {
                        root_path.join(p)
                    }
                } else {
                    // Nothing is written into the project unasked: the
                    // default location is the data directory.
                    let Ok(data_dir) = crate::core::data_dir::sobre_data_dir() else {
                        return "ERROR: no data directory to write the bundle to; pass path="
                            .to_string();
                    };
                    data_dir.join("exports").join("sessions").join(format!(
                        "ccp-session-bundle-v1_{}_{}.json",
                        bundle.session.id, ts
                    ))
                };

                if let Err(e) = crate::core::ccp_session_bundle::write_bundle_v1(&candidate, &json)
                {
                    return format!("ERROR: export write failed: {e}");
                }
                written = Some(candidate.to_string_lossy().to_string());
            }

            match format {
                "summary" => {
                    let mut out = format!(
                        "CCP session bundle exported (v{}).\n\
schema_version: {}\n\
session_id: {}\n\
bytes: {}\n",
                        bundle.session.version,
                        bundle.schema_version,
                        bundle.session.id,
                        json.len()
                    );
                    if let Some(p) = written {
                        out.push_str(&format!("path: {p}\n"));
                    }
                    if let Some(h) = bundle.project.project_root_hash {
                        out.push_str(&format!("project_root_hash: {h}\n"));
                    }
                    out
                }
                _ => {
                    if let Some(p) = written {
                        format!("{json}\n\npath: {p}")
                    } else {
                        json
                    }
                }
            }
        }

        "import" => {
            // The session's own root, never the server's cwd.
            let Some(root) = session
                .project_root
                .clone()
                .or_else(|| session.shell_cwd.clone())
            else {
                return "ERROR: import needs a project root; this session has none".to_string();
            };
            let root_path = std::path::PathBuf::from(&root);

            let Some(p) = opts.path.or(value) else {
                return "ERROR: path is required for action=import".to_string();
            };

            let candidate = {
                let p = std::path::PathBuf::from(p);
                if p.is_absolute() {
                    p
                } else {
                    root_path.join(p)
                }
            };
            let path = candidate;

            let bundle = match crate::core::ccp_session_bundle::read_bundle_v1(&path) {
                Ok(b) => b,
                Err(e) => return format!("ERROR: import failed: {e}"),
            };

            // Replayability hint: the bundle names the project it came from.
            let current_root_hash = crate::core::project_hash::hash_project_root(&root);

            let mut warning: Option<String> = None;
            if let Some(ref exported) = bundle.project.project_root_hash
                && exported != &current_root_hash
            {
                warning = Some(
                    "WARNING: project_root_hash mismatch (importing into different project root)."
                        .to_string(),
                );
            }

            if let Err(e) = save_if_recorded(session) {
                return format!(
                    "ERROR: the current session could not be saved ({e}); nothing imported"
                );
            }
            let report = crate::core::ccp_session_bundle::import_bundle_v1_into_session(
                session,
                &bundle,
                Some(&root),
            );
            if let Err(e) = session.save() {
                warning = Some(format!("WARNING: imported, but not saved: {e}"));
            }

            let mut out = format!(
                "CCP session bundle imported.\n\
session_id: {}\n\
version: {}\n\
files_touched: {}\n\
stale_files: {}\n",
                report.session_id, report.version, report.files_touched, report.stale_files
            );
            if let Some(w) = warning {
                out.push_str(&format!("{w}\n"));
            }
            out
        }

        "task" | "finding" | "decision" => {
            let Some(text) = value.map(str::trim).filter(|v| !v.is_empty()) else {
                return format!("ERROR: value is required for action={action}");
            };
            match action {
                "task" => {
                    session.set_task(text, None);
                    format!("Task set: {text}")
                }
                "finding" => {
                    let (file, line, summary) = parse_finding_value(text);
                    session.add_finding(file.as_deref(), line, summary);
                    format!("Finding added: {text}")
                }
                _ => {
                    session.add_decision(text, None);
                    format!("Decision recorded: {text}")
                }
            }
        }

        "reset" => {
            if let Err(e) = save_if_recorded(session) {
                return format!(
                    "ERROR: the current session could not be saved ({e}); nothing reset"
                );
            }
            let old_id = session.id.clone();
            session.replace_record(SessionState::new());
            format!("Session reset. Previous: {old_id}. New: {}.", session.id)
        }

        "list" => {
            // This project's sessions only, as `load` and `restore` see them.
            let same_root = |root: Option<&str>| match (root, session.project_root.as_deref()) {
                (Some(a), Some(b)) => {
                    crate::core::pathutil::safe_canonicalize_or_self(std::path::Path::new(a))
                        == crate::core::pathutil::safe_canonicalize_or_self(std::path::Path::new(b))
                }
                _ => false,
            };
            let sessions: Vec<_> = SessionState::list_sessions()
                .into_iter()
                .filter(|s| same_root(s.project_root.as_deref()))
                .collect();
            if sessions.is_empty() {
                return "No sessions found for this project.".to_string();
            }
            let mut lines = vec![format!("Sessions ({}):", sessions.len())];
            for s in sessions.iter().take(10) {
                let task = s.task.as_deref().unwrap_or("(no task)");
                let task_short: String = task.chars().take(40).collect();
                lines.push(format!(
                    "  {} v{} | {} calls | {} tok | {}",
                    s.id, s.version, s.tool_calls, s.tokens_saved, task_short
                ));
            }
            if sessions.len() > 10 {
                lines.push(format!("  ... +{} more", sessions.len() - 10));
            }
            lines.join("\n")
        }

        "cleanup" => {
            let removed = SessionState::cleanup_old_sessions(7);
            format!("Cleaned up {removed} old session(s) (>7 days).")
        }

        "snapshot" => match session.save_compaction_snapshot() {
            Ok(snapshot) => {
                format!(
                    "Compaction snapshot saved ({} bytes).\n{snapshot}",
                    snapshot.len()
                )
            }
            Err(e) => format!("Snapshot failed: {e}"),
        },

        "restore" => {
            let snapshot = if let Some(id) = session_id {
                SessionState::load_compaction_snapshot(id)
            } else {
                session
                    .project_root
                    .as_deref()
                    .and_then(SessionState::load_latest_snapshot)
            };
            match snapshot {
                Some(s) => format!("Session restored from compaction snapshot:\n{s}"),
                None => "ERROR: no compaction snapshot found".to_string(),
            }
        }

        "resume" => session.build_resume_block(),

        "diff" => {
            let parts: Vec<&str> = value.unwrap_or("").split_whitespace().collect();
            if parts.len() < 2 {
                return "Usage: sobre_session diff <session_id_a> <session_id_b> [format]\n\
                        Formats: summary (default), json\n\
                        Example: sobre_session diff abc123 def456 json"
                    .to_string();
            }
            let id_a = parts[0];
            let id_b = parts[1];
            let format = parts.get(2).copied().unwrap_or("summary");

            let sess_a = SessionState::load_by_id(id_a);
            let sess_b = SessionState::load_by_id(id_b);

            match (sess_a, sess_b) {
                (Some(a), Some(b)) => {
                    let d = crate::core::session_diff::diff_sessions(&a, &b);
                    match format {
                        "json" => d.format_json(),
                        _ => d.format_summary(),
                    }
                }
                (None, _) => format!("Session not found: {id_a}"),
                (_, None) => format!("Session not found: {id_b}"),
            }
        }

        "output_stats" => crate::core::output_verification::stats_snapshot().format_compact(),

        _ => format!(
            "Unknown action: {action}. Use: status, load, save, task, finding, decision, reset, list, cleanup, snapshot, restore, resume, diff, output_stats, export, import"
        ),
    }
}

fn parse_finding_value(value: &str) -> (Option<String>, Option<u32>, &str) {
    const EM_DASH_SEP: &str = " \u{2014} ";
    const ASCII_SEP: &str = " - ";

    let (dash_pos, sep) = if let Some(p) = value.find(EM_DASH_SEP) {
        (Some(p), EM_DASH_SEP)
    } else {
        (value.find(ASCII_SEP), ASCII_SEP)
    };

    if let Some(pos) = dash_pos {
        let location = &value[..pos];
        let text = &value[pos + sep.len()..];

        if let Some(colon_pos) = location.rfind(':') {
            let file = &location[..colon_pos];
            if let Ok(line) = location[colon_pos + 1..].parse::<u32>() {
                return (Some(file.to_string()), Some(line), text);
            }
        }
        return (Some(location.to_string()), None, text);
    }
    (None, None, value)
}

#[cfg(test)]
mod tests {
    use super::{SessionState, SessionToolOptions, handle, parse_finding_value};

    fn run(session: &mut SessionState, action: &str, value: Option<&str>) -> String {
        let opts = SessionToolOptions {
            format: None,
            path: None,
            write: false,
        };
        handle(session, action, value, None, opts)
    }

    /// L4: `reset` keeps the project root, the shell directory and the extra
    /// roots; only the record is new.
    #[test]
    fn reset_keeps_where_the_process_works() {
        let _data = crate::core::data_dir::isolated_data_dir();
        let mut session = SessionState::new();
        session.project_root = Some("/work/app".to_string());
        session.shell_cwd = Some("/work/app/src".to_string());
        session.extra_roots = vec!["/work/lib".to_string()];
        run(&mut session, "task", Some("ship it"));

        let out = run(&mut session, "reset", None);
        assert!(out.starts_with("Session reset."), "{out}");
        assert!(session.task.is_none());
        assert_eq!(session.project_root.as_deref(), Some("/work/app"));
        assert_eq!(session.shell_cwd.as_deref(), Some("/work/app/src"));
        assert_eq!(session.extra_roots, vec!["/work/lib".to_string()]);
    }

    /// L18: an empty value is refused, never stored as a placeholder.
    #[test]
    fn a_record_without_a_value_is_refused() {
        let mut session = SessionState::new();
        for action in ["task", "finding", "decision"] {
            let out = run(&mut session, action, Some("  "));
            assert!(out.starts_with("ERROR: value is required"), "{out}");
        }
        assert!(!session.has_record());
    }

    #[test]
    fn finding_with_em_dash_and_file_line() {
        let (file, line, text) =
            parse_finding_value("auth.rs:42 \u{2014} missing token validation");
        assert_eq!(file.as_deref(), Some("auth.rs"));
        assert_eq!(line, Some(42));
        assert_eq!(text, "missing token validation");
    }

    #[test]
    fn finding_with_ascii_dash_and_file_line() {
        let (file, line, text) = parse_finding_value("auth.rs:42 - missing token validation");
        assert_eq!(file.as_deref(), Some("auth.rs"));
        assert_eq!(line, Some(42));
        assert_eq!(text, "missing token validation");
    }

    #[test]
    fn finding_with_em_dash_no_line() {
        let (file, line, text) = parse_finding_value("auth module \u{2014} needs refactoring");
        assert_eq!(file.as_deref(), Some("auth module"));
        assert_eq!(line, None);
        assert_eq!(text, "needs refactoring");
    }

    #[test]
    fn finding_plain_text() {
        let (file, line, text) = parse_finding_value("plain text finding");
        assert_eq!(file, None);
        assert_eq!(line, None);
        assert_eq!(text, "plain text finding");
    }

    #[test]
    fn finding_cyrillic_with_em_dash() {
        let value = "ruff: pyproject.toml dev-group \u{2014} >=0.15.14,<0.16.0 (был 0.14.x)";
        let (file, line, text) = parse_finding_value(value);
        assert_eq!(file.as_deref(), Some("ruff: pyproject.toml dev-group"));
        assert_eq!(line, None);
        assert_eq!(text, ">=0.15.14,<0.16.0 (был 0.14.x)");
    }
}
