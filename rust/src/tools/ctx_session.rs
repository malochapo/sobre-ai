use crate::core::session::SessionState;

#[derive(Clone, Copy, Debug)]
pub struct SessionToolOptions<'a> {
    pub format: Option<&'a str>,
    pub path: Option<&'a str>,
    pub write: bool,
    pub privacy: Option<&'a str>,
}

pub fn handle(
    session: &mut SessionState,
    action: &str,
    value: Option<&str>,
    session_id: Option<&str>,
    opts: SessionToolOptions<'_>,
) -> String {
    match action {
        // "show" is the natural synonym agents reach for first (#658).
        "status" | "show" => session.format_compact(),

        "load" => {
            let loaded = if let Some(id) = session_id {
                SessionState::load_by_id(id)
            } else {
                SessionState::load_latest()
            };

            if let Some(prev) = loaded {
                let summary = prev.format_compact();
                *session = prev;
                format!("Session loaded.\n{summary}")
            } else {
                let id_str = session_id.unwrap_or("latest");
                format!("No session found (id: {id_str}). Starting fresh.")
            }
        }

        "save" => match session.save() {
            Ok(()) => format!("Session {} saved (v{}).", session.id, session.version),
            Err(e) => format!("Save failed: {e}"),
        },

        "export" => {
            let requested_privacy =
                crate::core::ccp_session_bundle::BundlePrivacyV1::parse(opts.privacy);
            if requested_privacy == crate::core::ccp_session_bundle::BundlePrivacyV1::Full {
                return "ERROR: privacy=full is not supported; exports are always redacted."
                    .to_string();
            }

            let bundle =
                crate::core::ccp_session_bundle::build_bundle_v1(session, requested_privacy);
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
                    root_path
                        .join(".lean-ctx")
                        .join("session_bundles")
                        .join(format!(
                            "ccp-session-bundle-v1_{}_{}.json",
                            bundle.session.id, ts
                        ))
                };

                if let Err(e) = crate::core::ccp_session_bundle::write_bundle_v1(&candidate, &json)
                {
                    return format!("Export write failed: {e}");
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
                    if let Some(h) = bundle.project.project_identity_hash {
                        out.push_str(&format!("project_identity_hash: {h}\n"));
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
            let root = session.project_root.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
            });
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
                Err(e) => return format!("Import failed: {e}"),
            };

            // Replayability hint: compare project identity hashes (best-effort).
            let current_root_hash = crate::core::project_hash::hash_project_root(&root);
            let current_identity_hash = crate::core::project_hash::project_identity(&root)
                .as_deref()
                .map(|s| {
                    use md5::{Digest, Md5};
                    let mut h = Md5::new();
                    h.update(s.as_bytes());
                    crate::core::hasher::hex_encode(&h.finalize())
                });

            let mut warning: Option<String> = None;
            if let Some(ref exported) = bundle.project.project_root_hash
                && exported != &current_root_hash
            {
                warning = Some(
                    "WARNING: project_root_hash mismatch (importing into different project root)."
                        .to_string(),
                );
            }
            if let (Some(exported), Some(current)) = (
                bundle.project.project_identity_hash.as_ref(),
                current_identity_hash.as_ref(),
            ) && exported != current
            {
                warning = Some("WARNING: project_identity_hash mismatch (importing into different project identity).".to_string());
            }

            let report = crate::core::ccp_session_bundle::import_bundle_v1_into_session(
                session,
                &bundle,
                Some(&root),
            );
            let _ = session.save();

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

        "task" => {
            let desc = value.unwrap_or("(no description)");
            session.set_task(desc, None);
            format!("Task set: {desc}")
        }

        "finding" => {
            let summary = value.unwrap_or("(no summary)");
            let (file, line, text) = parse_finding_value(summary);
            session.add_finding(file.as_deref(), line, text);
            format!("Finding added: {summary}")
        }

        "decision" => {
            let desc = value.unwrap_or("(no description)");
            session.add_decision(desc, None);
            format!("Decision recorded: {desc}")
        }

        "reset" => {
            let _ = session.save();
            let old_id = session.id.clone();
            *session = SessionState::new();
            // Clear the persistent context ledger so pressure resets to 0%
            let mut ledger = crate::core::context_ledger::ContextLedger::load();
            ledger.reset();
            ledger.save();
            if let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() {
                let radar_path = data_dir.join("context_radar.jsonl");
                let prev = data_dir.join("context_radar.prev.jsonl");
                let _ = std::fs::rename(&radar_path, &prev);
            }
            format!(
                "Session reset. Previous: {old_id}. New: {}. Ledger cleared (0% pressure).",
                session.id
            )
        }

        "list" => {
            let sessions = SessionState::list_sessions();
            if sessions.is_empty() {
                return "No sessions found.".to_string();
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
                SessionState::load_latest_snapshot()
            };
            match snapshot {
                Some(s) => format!("Session restored from compaction snapshot:\n{s}"),
                None => "No compaction snapshot found. Session continues fresh.".to_string(),
            }
        }

        "resume" => session.build_resume_block(),

        "diff" => {
            let parts: Vec<&str> = value.unwrap_or("").split_whitespace().collect();
            if parts.len() < 2 {
                return "Usage: ctx_session diff <session_id_a> <session_id_b> [format]\n\
                        Formats: summary (default), json\n\
                        Example: ctx_session diff abc123 def456 json"
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
    use super::parse_finding_value;

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
    fn finding_cyrillic_with_em_dash_issue_272() {
        let value = "ruff: pyproject.toml dev-group \u{2014} >=0.15.14,<0.16.0 (был 0.14.x)";
        let (file, line, text) = parse_finding_value(value);
        assert_eq!(file.as_deref(), Some("ruff: pyproject.toml dev-group"));
        assert_eq!(line, None);
        assert_eq!(text, ">=0.15.14,<0.16.0 (был 0.14.x)");
    }
}
