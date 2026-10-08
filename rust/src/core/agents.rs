use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistry {
    pub agents: Vec<AgentEntry>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub agent_id: String,
    pub agent_type: String,
    pub role: Option<String>,
    pub project_root: String,
    pub started_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub pid: u32,
    pub status: AgentStatus,
    pub status_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Active,
    Idle,
    Finished,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Active => write!(f, "active"),
            AgentStatus::Idle => write!(f, "idle"),
            AgentStatus::Finished => write!(f, "finished"),
        }
    }
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            updated_at: Utc::now(),
        }
    }

    pub fn register(&mut self, agent_type: &str, role: Option<&str>, project_root: &str) -> String {
        self.register_process(agent_type, role, project_root, std::process::id())
    }

    fn register_process(
        &mut self,
        agent_type: &str,
        role: Option<&str>,
        project_root: &str,
        pid: u32,
    ) -> String {
        let agent_id = format!("{}-{}-{}", agent_type, pid, generate_short_id());

        if let Some(existing) = self.agents.iter_mut().find(|a| a.pid == pid) {
            existing.last_active = Utc::now();
            existing.status = AgentStatus::Active;
            existing.agent_type = agent_type.to_string();
            existing.project_root = project_root.to_string();
            if let Some(r) = role {
                existing.role = Some(r.to_string());
            }
            return existing.agent_id.clone();
        }

        self.agents.push(AgentEntry {
            agent_id: agent_id.clone(),
            agent_type: agent_type.to_string(),
            role: role.map(std::string::ToString::to_string),
            project_root: project_root.to_string(),
            started_at: Utc::now(),
            last_active: Utc::now(),
            pid,
            status: AgentStatus::Active,
            status_message: None,
        });

        self.updated_at = Utc::now();
        agent_id
    }

    /// Atomically registers this MCP process in the shared on-disk registry.
    pub fn register_mcp_process(project_root: &str) -> Result<String, String> {
        mutate_persistent(|registry| {
            registry.cleanup_stale(24);
            registry.register("mcp", Some("context-engine"), project_root)
        })
    }

    /// Atomically refreshes a registered MCP process heartbeat.
    pub fn heartbeat_persistent(agent_id: &str) -> Result<(), String> {
        mutate_persistent(|registry| registry.update_heartbeat(agent_id))
    }

    /// Atomically marks a registered MCP process as finished.
    pub fn finish_persistent(agent_id: &str) -> Result<(), String> {
        mutate_persistent(|registry| {
            registry.set_status(agent_id, AgentStatus::Finished, Some("connection closed"));
        })
    }

    pub fn update_heartbeat(&mut self, agent_id: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            agent.last_active = Utc::now();
        }
    }

    pub fn set_status(&mut self, agent_id: &str, status: AgentStatus, message: Option<&str>) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            agent.status = status;
            agent.status_message = message.map(std::string::ToString::to_string);
            agent.last_active = Utc::now();
        }
        self.updated_at = Utc::now();
    }
    pub fn list_active(&self, project_root: Option<&str>) -> Vec<&AgentEntry> {
        self.agents
            .iter()
            .filter(|a| {
                if let Some(root) = project_root {
                    a.project_root == root && a.status != AgentStatus::Finished
                } else {
                    a.status != AgentStatus::Finished
                }
            })
            .collect()
    }

    pub fn cleanup_stale(&mut self, max_age_hours: u64) {
        let cutoff = Utc::now() - chrono::Duration::hours(max_age_hours as i64);

        for agent in &mut self.agents {
            if agent.status == AgentStatus::Finished {
                continue;
            }
            if !is_process_alive(agent.pid) {
                agent.status = AgentStatus::Finished;
            }
        }

        // Remove finished agents older than the cutoff to keep recent history visible.
        self.agents
            .retain(|a| !(a.status == AgentStatus::Finished && a.last_active < cutoff));

        self.updated_at = Utc::now();
    }

    #[cfg(test)]
    pub fn save(&self) -> Result<(), String> {
        let dir = agents_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let lock_path = dir.join("registry.lock");
        let _lock = FileLock::acquire(&lock_path)?;

        self.save_locked(&dir)
    }

    fn save_locked(&self, dir: &std::path::Path) -> Result<(), String> {
        let path = dir.join("registry.json");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }

    pub fn load() -> Option<Self> {
        let dir = agents_dir().ok()?;
        let path = dir.join("registry.json");
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    #[cfg(test)]
    pub fn load_or_create() -> Self {
        Self::load().unwrap_or_default()
    }

    /// Atomically load, mutate, and persist the registry under a single file
    /// lock. `load_or_create()` + mutate + `save()` is a read-modify-write
    /// race: `save()` only locks the final write, so two concurrent callers
    /// (two MCP sessions registering, or the dashboard's own poll-triggered
    /// `cleanup_stale` + save) can each load a stale snapshot and the last
    /// writer silently drops the other's changes — e.g. a second session's
    /// registration vanishing from the dashboard. Holding the lock across
    /// the re-read closes that window: the read inside always sees the
    /// latest on-disk state.
    pub fn mutate_locked<T>(f: impl FnOnce(&mut Self) -> T) -> Result<(Self, T), String> {
        let dir = agents_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let lock_path = dir.join("registry.lock");
        let _lock = FileLock::acquire(&lock_path)?;

        let mut registry = Self::load().unwrap_or_default();
        let out = f(&mut registry);
        registry.save_locked(&dir)?;
        Ok((registry, out))
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn agents_dir() -> Result<PathBuf, String> {
    let dir = crate::core::data_dir::sobre_data_dir()?;
    Ok(dir.join("agents"))
}

fn mutate_persistent<T>(mutate: impl FnOnce(&mut AgentRegistry) -> T) -> Result<T, String> {
    let dir = agents_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _lock = FileLock::acquire(&dir.join("registry.lock"))?;
    let path = dir.join("registry.json");
    let mut registry = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();
    let result = mutate(&mut registry);
    let json = serde_json::to_string_pretty(&registry).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(result)
}

fn generate_short_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

/// #576 already fixed this exact hardcoded-`true` anti-pattern for
/// `daemon::is_daemon_running` by delegating to `ipc::process::is_alive`
/// (which has a real Windows `OpenProcess` check); this duplicate copy was
/// missed, so on non-unix targets `cleanup_stale` could never flip a dead
/// MCP session's entry to `Finished`, leaving `registry.json` accumulating
/// stale `Active` entries forever — the root cause of the "N active agents"
/// dashboard bug on Windows.
pub fn is_process_alive(pid: u32) -> bool {
    crate::ipc::process::is_alive(pid)
}

pub(crate) struct FileLock {
    path: PathBuf,
}

impl FileLock {
    pub(crate) fn acquire(path: &std::path::Path) -> Result<Self, String> {
        for _ in 0..50 {
            if std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .is_ok()
            {
                return Ok(Self {
                    path: path.to_path_buf(),
                });
            }
            if let Ok(metadata) = std::fs::metadata(path)
                && let Ok(modified) = metadata.modified()
                && modified.elapsed().unwrap_or_default().as_secs() > 5
            {
                let _ = std::fs::remove_file(path);
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Err("Could not acquire lock after 5 seconds".to_string())
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_list() {
        let mut reg = AgentRegistry::new();
        let id = reg.register("cursor", Some("dev"), "/tmp/project");
        assert!(!id.is_empty());
        assert_eq!(reg.list_active(None).len(), 1);
        assert_eq!(reg.list_active(None)[0].agent_type, "cursor");
    }

    #[test]
    fn reregister_same_pid() {
        let mut reg = AgentRegistry::new();
        let id1 = reg.register("cursor", Some("dev"), "/tmp/project");
        let id2 = reg.register("cursor", Some("review"), "/tmp/project");
        assert_eq!(id1, id2);
        assert_eq!(reg.agents.len(), 1);
        assert_eq!(reg.agents[0].role, Some("review".to_string()));
    }

    #[test]
    fn set_status() {
        let mut reg = AgentRegistry::new();
        let id = reg.register("claude", None, "/tmp/project");
        reg.set_status(&id, AgentStatus::Idle, Some("waiting for review"));
        assert_eq!(reg.agents[0].status, AgentStatus::Idle);
        assert_eq!(
            reg.agents[0].status_message,
            Some("waiting for review".to_string())
        );
    }

    fn test_entry(agent_id: &str, project_root: &str, pid: u32) -> AgentEntry {
        let now = Utc::now();
        AgentEntry {
            agent_id: agent_id.to_string(),
            agent_type: "cursor".to_string(),
            role: Some("dev".to_string()),
            project_root: project_root.to_string(),
            started_at: now,
            last_active: now,
            pid,
            status: AgentStatus::Active,
            status_message: None,
        }
    }

    /// #419: the wake-up briefing scopes agents to the current project via
    /// `list_active(Some(root))`. Peers working on *other* projects must never
    /// leak into the briefing.
    #[test]
    fn list_active_scopes_to_project_root() {
        let mut reg = AgentRegistry::new();
        reg.agents
            .push(test_entry("a-1", "/proj/a", std::process::id()));
        reg.agents
            .push(test_entry("b-1", "/proj/b", std::process::id()));

        let active_a = reg.list_active(Some("/proj/a"));
        assert_eq!(active_a.len(), 1);
        assert_eq!(active_a[0].agent_id, "a-1");

        // Unscoped still sees both.
        assert_eq!(reg.list_active(None).len(), 2);
    }

    /// #419: a crashed/exited MCP process leaves an `Active` entry behind.
    /// `cleanup_stale` must flip it to `Finished` (regardless of age) so
    /// `list_active` no longer surfaces it as a live peer — the ghost the
    /// briefing used to show. Previously `#[cfg(unix)]`-only, which is why
    /// the non-unix `is_process_alive` hardcoded-`true` regression (see its
    /// doc comment) shipped unnoticed: this exact test never ran on Windows.
    #[test]
    fn cleanup_stale_prunes_dead_pid_from_active_list() {
        // Reap a child so its PID is guaranteed dead at assertion time.
        let reaped = {
            let mut cmd = if cfg!(windows) {
                let mut c = std::process::Command::new("cmd");
                c.args(["/C", "exit"]);
                c
            } else {
                std::process::Command::new("true")
            };
            let mut child = cmd.spawn().expect("spawn short-lived helper process");
            let pid = child.id();
            child.wait().expect("reap helper process");
            pid
        };

        let mut reg = AgentRegistry::new();
        reg.agents.push(test_entry("ghost", "/proj/a", reaped));
        reg.agents
            .push(test_entry("live", "/proj/a", std::process::id()));

        reg.cleanup_stale(24);

        let ids: Vec<&str> = reg
            .list_active(Some("/proj/a"))
            .iter()
            .map(|a| a.agent_id.as_str())
            .collect();
        assert!(ids.contains(&"live"), "live same-project agent must remain");
        assert!(
            !ids.contains(&"ghost"),
            "dead-pid agent must be pruned from the active list (#419)"
        );
    }

    /// Regression: concurrent load-mutate-save cycles must not silently drop
    /// each other's changes. Before `mutate_locked`, `save()` only locked the
    /// final write — the preceding `load()` was unlocked, so a second writer
    /// could load a stale snapshot and overwrite the first writer's addition
    /// (e.g. a second Claude Code session's agent registration vanishing
    /// from the dashboard).
    #[test]
    fn mutate_locked_survives_concurrent_writers() {
        let _iso = crate::core::data_dir::isolated_data_dir();

        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    AgentRegistry::mutate_locked(|registry| {
                        registry.agents.push(AgentEntry {
                            agent_id: format!("agent-{i}"),
                            agent_type: "test".to_string(),
                            role: None,
                            project_root: "/tmp/project".to_string(),
                            started_at: Utc::now(),
                            last_active: Utc::now(),
                            pid: 10_000 + i,
                            status: AgentStatus::Active,
                            status_message: None,
                        });
                    })
                })
            })
            .collect();

        for h in handles {
            h.join()
                .expect("writer thread must not panic")
                .expect("mutate_locked must succeed");
        }

        let registry = AgentRegistry::load_or_create();
        assert_eq!(
            registry.agents.len(),
            8,
            "all 8 concurrent registrations must survive, got {}",
            registry.agents.len()
        );
    }
}

#[cfg(test)]
mod presence_tests {
    use super::*;

    #[test]
    fn persistent_presence_preserves_multiple_processes_and_lifecycle() {
        let isolated = crate::core::data_dir::isolated_data_dir();
        let mut registry = AgentRegistry::new();
        let first = registry.register_process("mcp", Some("context-engine"), "/project", 101);
        let second = registry.register_process("mcp", Some("context-engine"), "/project", 202);
        registry.save().expect("save registry");

        assert_ne!(first, second);
        assert_eq!(AgentRegistry::load().expect("registry").agents.len(), 2);

        AgentRegistry::heartbeat_persistent(&first).expect("heartbeat");
        AgentRegistry::finish_persistent(&second).expect("finish");
        let loaded = AgentRegistry::load().expect("registry");
        assert_eq!(
            loaded
                .agents
                .iter()
                .find(|agent| agent.agent_id == second)
                .expect("second agent")
                .status,
            AgentStatus::Finished
        );
        assert!(isolated.path().join("agents/registry.json").exists());
    }

    #[test]
    fn reregistering_process_refreshes_metadata_without_duplication() {
        let mut registry = AgentRegistry::new();
        let first = registry.register_process("unknown", None, "/old", 303);
        let second = registry.register_process("mcp", Some("context-engine"), "/new", 303);

        assert_eq!(first, second);
        assert_eq!(registry.agents.len(), 1);
        assert_eq!(registry.agents[0].agent_type, "mcp");
        assert_eq!(registry.agents[0].project_root, "/new");
        assert_eq!(registry.agents[0].role.as_deref(), Some("context-engine"));
    }
}
