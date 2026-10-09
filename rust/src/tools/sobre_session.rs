use crate::core::session::SessionState;

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
) -> String {
    match action {
        "status" => session.format_compact(),

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

        "resume" => session.build_resume_block(),

        _ => format!(
            "Unknown action: {action}. Use: status, task, finding, decision, load, list, resume"
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
    use super::{SessionState, handle, parse_finding_value};

    fn run(session: &mut SessionState, action: &str, value: Option<&str>) -> String {
        handle(session, action, value, None)
    }

    /// An empty value is refused, never stored as a placeholder.
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
