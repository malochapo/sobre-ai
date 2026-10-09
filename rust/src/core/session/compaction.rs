use crate::core::config::CompressionLevel;

use super::paths::shorten_path;
use super::types::SessionState;

/// Format a resume block hint for session restore.
fn resume_block_hint(level: &CompressionLevel) -> Option<String> {
    match level {
        CompressionLevel::Off => None,
        CompressionLevel::Lite => Some(
            "[COMPRESSION: lite] Keep responses concise. Bullet points, avoid filler.".to_string(),
        ),
        CompressionLevel::Standard => Some(
            "[COMPRESSION: standard] Dense output. Atomic fact lines, abbreviations, diff-only code.".to_string(),
        ),
        CompressionLevel::Max | CompressionLevel::Raw => Some(
            "[COMPRESSION: max] Expert-terse mode. Telegraph format, symbolic vocabulary, zero narration.".to_string(),
        ),
    }
}

impl SessionState {
    /// Formats the session state as a compact multi-line summary for agent context.
    pub fn format_compact(&self) -> String {
        let duration = self.updated_at - self.started_at;
        let hours = duration.num_hours();
        let mins = duration.num_minutes() % 60;
        let duration_str = if hours > 0 {
            format!("{hours}h {mins}m")
        } else {
            format!("{mins}m")
        };

        let mut lines = Vec::new();
        lines.push(format!(
            "SESSION v{} | {} | {} calls | {} tok saved",
            self.version, duration_str, self.stats.total_tool_calls, self.stats.total_tokens_saved
        ));

        if let Some(ref task) = self.task {
            lines.push(format!("Task: {}", task.description));
        }

        if let Some(ref root) = self.project_root {
            lines.push(format!("Root: {}", shorten_path(root)));
        }

        if !self.findings.is_empty() {
            let items: Vec<String> = self
                .findings
                .iter()
                .rev()
                .take(5)
                .map(|f| {
                    let loc = match (&f.file, f.line) {
                        (Some(file), Some(line)) => format!("{}:{line}", shorten_path(file)),
                        (Some(file), None) => shorten_path(file),
                        _ => String::new(),
                    };
                    if loc.is_empty() {
                        f.summary.clone()
                    } else {
                        format!("{loc} \u{2014} {}", f.summary)
                    }
                })
                .collect();
            lines.push(format!(
                "Findings ({}): {}",
                self.findings.len(),
                items.join(" | ")
            ));
        }

        if !self.decisions.is_empty() {
            let items: Vec<&str> = self
                .decisions
                .iter()
                .rev()
                .take(3)
                .map(|d| d.summary.as_str())
                .collect();
            lines.push(format!("Decisions: {}", items.join(" | ")));
        }

        if !self.files_touched.is_empty() {
            let items: Vec<String> = self
                .files_touched
                .iter()
                .rev()
                .take(10)
                .map(|f| {
                    let status = if f.modified { "mod" } else { &f.last_mode };
                    let r = f.file_ref.as_deref().unwrap_or("?");
                    format!("[{r} {} {status}]", shorten_path(&f.path))
                })
                .collect();
            lines.push(format!(
                "Files ({}): {}",
                self.files_touched.len(),
                items.join(" ")
            ));
        }

        lines.join("\n")
    }

    /// Build a compact resume block for post-compaction injection.
    /// Max ~500 tokens. Includes task, decisions and files.
    pub fn build_resume_block(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        let level =
            crate::core::config::CompressionLevel::effective(&crate::core::config::Config::load());
        if let Some(hint) = resume_block_hint(&level) {
            parts.push(hint);
        }

        if let Some(ref root) = self.project_root {
            let short = root.rsplit('/').next().unwrap_or(root);
            parts.push(format!("Project: {short}"));
        }

        if let Some(ref task) = self.task {
            parts.push(format!("Task: {}", task.description));
        }

        if !self.decisions.is_empty() {
            let items: Vec<&str> = self
                .decisions
                .iter()
                .rev()
                .take(5)
                .map(|d| d.summary.as_str())
                .collect();
            parts.push(format!("Decisions: {}", items.join("; ")));
        }

        if !self.files_touched.is_empty() {
            let modified: Vec<String> = self
                .files_touched
                .iter()
                .filter(|f| f.modified)
                .take(10)
                .map(|f| {
                    f.summary
                        .as_deref()
                        .map_or_else(|| f.path.clone(), |s| format!("{} ({})", f.path, s))
                })
                .collect();
            if !modified.is_empty() {
                parts.push(format!("Modified: {}", modified.join(", ")));
            }
        }

        if !self.findings.is_empty() {
            let recent: Vec<&str> = self
                .findings
                .iter()
                .rev()
                .take(5)
                .map(|f| f.summary.as_str())
                .collect();
            parts.push(format!("Key findings: {}", recent.join("; ")));
        }

        parts.push(format!(
            "Stats: {} calls, {} tok saved",
            self.stats.total_tool_calls, self.stats.total_tokens_saved
        ));

        format!(
            "--- SESSION RESUME (post-compaction) ---\n{}\n---",
            parts.join("\n")
        )
    }
}
