use chrono::Utc;

use super::paths::{extract_cd_target, generate_session_id};
#[allow(clippy::wildcard_imports)]
use super::types::*;

const MAX_FINDINGS: usize = 20;
const MAX_DECISIONS: usize = 10;
const MAX_FILES: usize = 50;
pub(crate) const BATCH_SAVE_INTERVAL: u32 = 5;
/// Max time an unsaved change may linger before it is flushed even
/// below the batch threshold — `sobre_session load` and `list` read session
/// JSONs from disk, so a slow trickle of tool calls must still reach it
/// promptly.
pub(crate) const SESSION_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionState {
    /// Creates a new session with a unique ID and current timestamp.
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            id: generate_session_id(),
            version: 0,
            started_at: now,
            updated_at: now,
            project_root: None,
            shell_cwd: None,
            task: None,
            findings: Vec::new(),
            decisions: Vec::new(),
            files_touched: Vec::new(),
            stats: SessionStats::default(),
            extra_roots: Vec::new(),
            last_flush: None,
        }
    }

    /// Replace this session's record with `other`, keeping where this process
    /// works: the project root, the shell directory and the extra roots.
    pub fn replace_record(&mut self, other: Self) {
        let project_root = self.project_root.take();
        let shell_cwd = self.shell_cwd.take();
        let extra_roots = std::mem::take(&mut self.extra_roots);
        *self = other;
        self.project_root = project_root;
        self.shell_cwd = shell_cwd;
        self.extra_roots = extra_roots;
    }

    /// Whether the session holds anything worth saving: a task, findings,
    /// decisions or touched files.
    pub fn has_record(&self) -> bool {
        self.task.is_some()
            || !self.findings.is_empty()
            || !self.decisions.is_empty()
            || !self.files_touched.is_empty()
    }

    /// Bumps the version counter and marks the session as dirty.
    pub fn increment(&mut self) {
        self.version += 1;
        self.updated_at = Utc::now();
        self.stats.unsaved_changes += 1;
    }

    /// Returns `true` if enough changes have accumulated to warrant a disk
    /// save — or, if any change has waited longer than
    /// `SESSION_FLUSH_INTERVAL`: the 5-change batch alone kept slow
    /// sessions off disk for the whole batch window. A fresh in-memory session flushes its first change
    /// immediately so new activity surfaces at once.
    pub fn should_save(&self) -> bool {
        if self.stats.unsaved_changes == 0 {
            return false;
        }
        if self.stats.unsaved_changes >= BATCH_SAVE_INTERVAL {
            return true;
        }
        match self.last_flush {
            Some(t) => t.elapsed() >= SESSION_FLUSH_INTERVAL,
            None => true,
        }
    }

    /// Sets the task the agent stated (`sobre_session action=task`).
    pub fn set_task(&mut self, description: &str, intent: Option<&str>) {
        self.task = Some(TaskInfo {
            description: description.to_string(),
            intent: intent.map(std::string::ToString::to_string),
        });
        self.increment();
    }

    /// Records a finding (discovery or observation) in the session log.
    pub fn add_finding(&mut self, file: Option<&str>, line: Option<u32>, summary: &str) {
        let (summary_clean, _) = crate::core::secret_detection::scan_and_redact(summary);
        self.findings.push(Finding {
            file: file.map(std::string::ToString::to_string),
            line,
            summary: summary_clean,
            timestamp: Utc::now(),
        });
        while self.findings.len() > MAX_FINDINGS {
            self.findings.remove(0);
        }
        self.increment();
    }

    /// Records a design or implementation decision with optional rationale.
    pub fn add_decision(&mut self, summary: &str, rationale: Option<&str>) {
        let (summary_clean, _) = crate::core::secret_detection::scan_and_redact(summary);
        let rationale_clean =
            rationale.map(|r| crate::core::secret_detection::scan_and_redact(r).0);
        self.decisions.push(Decision {
            summary: summary_clean,
            rationale: rationale_clean,
            timestamp: Utc::now(),
        });
        while self.decisions.len() > MAX_DECISIONS {
            self.decisions.remove(0);
        }
        self.increment();
    }

    /// Records a file read/access in the session, incrementing its read count.
    pub fn touch_file(&mut self, path: &str, file_ref: Option<&str>, mode: &str, tokens: usize) {
        if let Some(existing) = self.files_touched.iter_mut().find(|f| f.path == path) {
            existing.read_count += 1;
            existing.last_mode = mode.to_string();
            existing.tokens = tokens;
            if let Some(r) = file_ref {
                existing.file_ref = Some(r.to_string());
            }
        } else {
            self.files_touched.push(FileTouched {
                path: path.to_string(),
                file_ref: file_ref.map(std::string::ToString::to_string),
                read_count: 1,
                modified: false,
                last_mode: mode.to_string(),
                tokens,
                stale: false,
                summary: None,
            });
            while self.files_touched.len() > MAX_FILES {
                self.files_touched.remove(0);
            }
        }
        self.stats.files_read += 1;
        self.increment();
    }

    /// Marks a previously touched file as modified (written to).
    pub fn mark_modified(&mut self, path: &str) {
        if let Some(existing) = self.files_touched.iter_mut().find(|f| f.path == path) {
            existing.modified = true;
        }
        self.increment();
    }

    /// Sets a one-line content summary for a touched file (max 80 chars).
    pub fn set_file_summary(&mut self, path: &str, summary: &str) {
        if let Some(existing) = self.files_touched.iter_mut().find(|f| f.path == path) {
            let truncated = if summary.len() > 80 {
                format!("{}…", &summary[..summary.floor_char_boundary(79)])
            } else {
                summary.to_string()
            };
            existing.summary = Some(truncated);
        }
    }

    /// Increments the tool call counter and accumulates token savings.
    pub fn record_tool_call(&mut self, tokens_saved: u64) {
        self.stats.total_tool_calls += 1;
        self.stats.total_tokens_saved += tokens_saved;
    }

    /// Increments the session-level command counter.
    pub fn record_command(&mut self) {
        self.stats.commands_run += 1;
    }

    /// Returns the effective working directory for shell commands.
    /// Priority: explicit cwd arg (relative to the project root) > session
    /// shell_cwd > project_root > process cwd. sobre does not confine it: the
    /// agent host decides where commands may run (invariant 5).
    pub fn effective_cwd(&self, explicit_cwd: Option<&str>) -> String {
        if let Some(cwd) = explicit_cwd
            && !cwd.is_empty()
            && cwd != "."
        {
            let p = std::path::Path::new(cwd);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::path::Path::new(self.project_root.as_deref().unwrap_or(".")).join(p)
            };
            return crate::core::pathutil::safe_canonicalize_or_self(&abs)
                .to_string_lossy()
                .to_string();
        }
        if let Some(ref cwd) = self.shell_cwd {
            return cwd.clone();
        }
        if let Some(ref r) = self.project_root {
            return r.clone();
        }
        std::env::current_dir()
            .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
    }

    /// Persist an explicit `cwd` argument as the live shell
    /// cwd. `update_shell_cwd` only tracks `cd` inside the command
    /// text, but clients that switch checkouts mid-session (Claude Code
    /// after `EnterWorktree`) pass the new directory as the `cwd` *param* of
    /// every subsequent call — without persisting it, the worktree-divergence
    /// detection in path resolution never sees the switch.
    pub fn note_explicit_cwd(&mut self, cwd: &str) {
        let path = std::path::Path::new(cwd);
        if path.is_absolute() && path.is_dir() {
            self.shell_cwd = Some(
                crate::core::pathutil::safe_canonicalize_or_self(path)
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    /// Updates shell_cwd by detecting `cd` in the command.
    /// Handles: `cd /abs/path`, `cd rel/path` (relative to current cwd),
    /// `cd ..`, and chained commands like `cd foo && ...`.
    pub fn update_shell_cwd(&mut self, command: &str) {
        let base = self.effective_cwd(None);
        if let Some(new_cwd) = extract_cd_target(command, &base) {
            let path = std::path::Path::new(&new_cwd);
            if path.exists() && path.is_dir() {
                self.shell_cwd = Some(
                    crate::core::pathutil::safe_canonicalize_or_self(path)
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
    }
}
