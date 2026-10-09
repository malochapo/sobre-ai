use chrono::Utc;

use super::heuristics::{normalize_loaded_session, session_matches_project_root};
use super::paths::sessions_dir;
use super::state::BATCH_SAVE_INTERVAL;
#[allow(clippy::wildcard_imports)]
use super::types::*;

#[cfg(unix)]
fn restrict_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    let _ = std::fs::set_permissions(path, perms);
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &std::path::Path) {}

fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.starts_with('.')
        || id.contains('/')
        || id.contains('\\')
        || id.contains(std::path::MAIN_SEPARATOR)
    {
        return Err("invalid session id".to_string());
    }
    Ok(())
}

impl PreparedSave {
    /// Writes the pre-serialized session data and compaction snapshot to
    /// disk atomically.
    pub fn write_to_disk(self) -> Result<(), String> {
        if !self.dir.exists() {
            std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        }
        let path = self.dir.join(format!("{}.json", self.id));
        // A save can run on a background thread while another one is
        // prepared: each write gets its own temporary file.
        static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SAVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = self.dir.join(format!(
            ".{}.{}.{seq}.json.tmp",
            self.id,
            std::process::id()
        ));
        std::fs::write(&tmp, &self.json).map_err(|e| e.to_string())?;
        restrict_file_permissions(&tmp);
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;

        if let Some(snapshot) = self.compaction_snapshot {
            let snap_path = self.dir.join(format!("{}_snapshot.txt", self.id));
            let _ = std::fs::write(&snap_path, &snapshot);
            restrict_file_permissions(&snap_path);
        }
        Ok(())
    }
}

impl SessionState {
    /// Serializes and writes the session state to disk synchronously.
    pub fn save(&mut self) -> Result<(), String> {
        let prepared = self.prepare_save()?;
        match prepared.write_to_disk() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.stats.unsaved_changes = BATCH_SAVE_INTERVAL;
                Err(e)
            }
        }
    }

    /// Serialize session state while holding the lock (CPU-only), reset the
    /// unsaved counter, and return a `PreparedSave` whose I/O can be deferred
    /// to a background thread via `write_to_disk()`.
    pub fn prepare_save(&mut self) -> Result<PreparedSave, String> {
        let dir = sessions_dir().ok_or("cannot determine home directory")?;
        let compaction_snapshot = if self.stats.total_tool_calls > 0 {
            Some(self.build_compaction_snapshot())
        } else {
            None
        };
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        self.stats.unsaved_changes = 0;
        // Arm the time-based flush window.
        self.last_flush = Some(std::time::Instant::now());
        Ok(PreparedSave {
            dir,
            id: self.id.clone(),
            json,
            compaction_snapshot,
        })
    }

    /// Loads the most recent saved session of the current working directory's
    /// project, for `doctor` and `cleanup`. `None` for a broad or unsafe cwd
    /// (the MCP server's HOME, `/`).
    pub fn load_latest() -> Option<Self> {
        let cwd = std::env::current_dir().ok()?;
        if crate::core::pathutil::is_broad_or_unsafe_root(&cwd) {
            return None;
        }
        Self::load_latest_for_project_root(&cwd.to_string_lossy())
    }

    /// Loads the most recent session matching a specific project root.
    pub fn load_latest_for_project_root(project_root: &str) -> Option<Self> {
        Self::all_for_project_root(project_root)
            .into_iter()
            .max_by_key(|s| s.updated_at)
    }

    /// Loads every saved session that belongs to a project root.
    pub fn all_for_project_root(project_root: &str) -> Vec<Self> {
        // Broad roots ("/", HOME, agent sandboxes) never own a session. Bail out
        // BEFORE scanning: the MCP server boots with cwd "/" and previously walked
        // every stored session here, stat-ing each session's project_root /
        // shell_cwd. For roots under ~/Documents that probe popped the macOS
        // TCC prompt in sobre's name on every launchd (re)start —
        // and `shell_cwd.starts_with("/")` could even leak an arbitrary
        // project's session into the broad-root context.
        if crate::core::pathutil::is_broad_or_unsafe_root(std::path::Path::new(project_root)) {
            return Vec::new();
        }
        let Some(dir) = sessions_dir() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let target_root =
            crate::core::pathutil::safe_canonicalize_or_self(std::path::Path::new(project_root));
        let mut matches = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let Some(id) = path.file_stem().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(session) = Self::load_by_id(id) else {
                continue;
            };

            if session_matches_project_root(&session, &target_root) {
                matches.push(session);
            }
        }

        matches
    }

    /// Loads a specific session from disk by its unique ID.
    pub fn load_by_id(id: &str) -> Option<Self> {
        validate_session_id(id).ok()?;
        let dir = sessions_dir()?;
        let path = dir.join(format!("{id}.json"));
        let json = std::fs::read_to_string(&path).ok()?;
        let session: Self = serde_json::from_str(&json).ok()?;
        Some(normalize_loaded_session(session))
    }

    /// Lists all saved sessions as summaries, sorted by most recently updated.
    pub fn list_sessions() -> Vec<SessionSummary> {
        let Some(dir) = sessions_dir() else {
            return Vec::new();
        };

        let mut summaries = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(json) = std::fs::read_to_string(&path)
                    && let Ok(session) = serde_json::from_str::<SessionState>(&json)
                {
                    summaries.push(SessionSummary {
                        id: session.id,
                        updated_at: session.updated_at,
                        version: session.version,
                        task: session.task.as_ref().map(|t| t.description.clone()),
                        tool_calls: session.stats.total_tool_calls,
                        tokens_saved: session.stats.total_tokens_saved,
                        project_root: session.project_root,
                    });
                }
            }
        }

        summaries.sort_by_key(|x| std::cmp::Reverse(x.updated_at));
        summaries
    }

    /// Deletes sessions older than `max_age_days`, preserving the latest. Returns count removed.
    pub fn cleanup_old_sessions(max_age_days: i64) -> u32 {
        let Some(dir) = sessions_dir() else { return 0 };

        let cutoff = Utc::now() - chrono::Duration::days(max_age_days);
        let latest = Self::load_latest().map(|s| s.id);
        let mut removed = 0u32;

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let filename = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
                if filename.starts_with('.') {
                    continue;
                }
                if latest.as_deref() == Some(filename) {
                    continue;
                }
                if let Ok(json) = std::fs::read_to_string(&path)
                    && let Ok(session) = serde_json::from_str::<SessionState>(&json)
                    && session.updated_at < cutoff
                    && std::fs::remove_file(&path).is_ok()
                {
                    // Its compaction snapshot goes with it.
                    let _ = std::fs::remove_file(dir.join(format!("{filename}_snapshot.txt")));
                    removed += 1;
                }
            }
        }

        removed
    }
}
