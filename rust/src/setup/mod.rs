//! `sobre init`: connects agents (MCP entry, hooks, skill) and reports, per
//! agent, the files it wrote. `init --check` runs the same code with writes
//! turned off.

use std::path::{Path, PathBuf};

use crate::config_io::journal::{self, Change};
use crate::core::editor_registry::EditorTarget;
use crate::core::setup_report::{PlatformInfo, SetupItem, SetupReport, SetupStepReport};

pub mod skill;

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

/// What `init` did for one agent.
#[derive(Debug)]
pub struct AgentOutcome {
    pub name: &'static str,
    /// False when the agent is not installed and was not named: nothing was
    /// written for it.
    pub detected: bool,
    pub files: Vec<(PathBuf, Change)>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// The agent keys `init <agent>` accepts, with the alias each one maps to.
pub fn agent_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "claude" | "claude-code" => "claude",
        "codex" => "codex",
        "copilot" => "copilot",
        "cursor" => "cursor",
        "gemini" => "gemini",
        "opencode" => "opencode",
        "vscode" => "vscode",
        "windsurf" => "windsurf",
        "zed" => "zed",
        _ => return None,
    })
}

/// The agents that already carry something of sobre's, as `agent_key`s:
/// its MCP entry, its hooks, its skill or its plugin. What `doctor --fix`
/// repairs; connecting another agent is `init`'s job.
///
/// Copilot CLI and VS Code share their hooks and skill directory, so for
/// them only the MCP entry tells which one was connected.
pub fn agents_with_sobre(home: &Path) -> Vec<String> {
    crate::core::editor_registry::build_targets(home)
        .into_iter()
        .filter(|target| {
            has_mcp_entry(target) || own_footprint(&target.agent_key, home).is_some_and(|p| p)
        })
        .map(|target| target.agent_key)
        .collect()
}

/// Whether `target`'s MCP config holds a `sobre` server.
fn has_mcp_entry(target: &EditorTarget) -> bool {
    use crate::core::editor_registry::ConfigType;
    let Ok(content) = std::fs::read_to_string(&target.config_path) else {
        return false;
    };
    if target.config_type == ConfigType::Codex {
        return content
            .parse::<toml::Table>()
            .ok()
            .and_then(|t| t.get("mcp_servers")?.as_table()?.get("sobre").cloned())
            .is_some();
    }
    let container = match target.config_type {
        ConfigType::Zed => "context_servers",
        ConfigType::VsCodeMcp => "servers",
        ConfigType::OpenCode => "mcp",
        _ => "mcpServers",
    };
    crate::core::jsonc::parse_jsonc(&content)
        .ok()
        .and_then(|v| v.get(container)?.get("sobre").cloned())
        .is_some()
}

/// Whether the hooks, skill or plugin only `agent` gets from `init` are
/// there; `None` for an agent that has none of its own.
fn own_footprint(agent: &str, home: &Path) -> Option<bool> {
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    let hooks = match agent {
        "claude" => crate::core::editor_registry::claude_state_dir(home).join("settings.json"),
        "cursor" => home.join(".cursor/hooks.json"),
        "codex" => codex_dir.join("hooks.json"),
        "windsurf" => home.join(".codeium/windsurf/hooks.json"),
        "gemini" => home.join(".gemini/settings.json"),
        "opencode" => return Some(home.join(".config/opencode/plugins/sobre.js").exists()),
        _ => return None,
    };
    let skill = skill::skill_path(agent, home).is_some_and(|p| p.exists());
    Some(skill || holds_sobre_hook(&hooks))
}

/// Whether the JSON(C) file at `path` holds a string that runs sobre's hook.
fn holds_sobre_hook(path: &Path) -> bool {
    fn walk(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::String(s) => crate::hooks::is_sobre_hook_command(s),
            serde_json::Value::Array(items) => items.iter().any(walk),
            serde_json::Value::Object(map) => map.values().any(walk),
            _ => false,
        }
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| crate::core::jsonc::parse_jsonc(&c).ok())
        .is_some_and(|v| walk(&v))
}

/// Connects agents: the named ones (installed or not), or else every one that
/// is installed. Each gets its MCP entry, its hooks and its skill. With
/// `dry_run` nothing is written and the outcomes say what would be.
pub fn connect_agents(named: &[&str], dry_run: bool) -> Vec<AgentOutcome> {
    let Some(home) = crate::core::home::resolve_home_dir() else {
        return Vec::new();
    };
    let binary = crate::core::portable_binary::resolve_portable_binary();
    let targets = crate::core::editor_registry::build_targets(&home);

    journal::begin(dry_run);
    let outcomes = targets
        .iter()
        .filter(|t| named.is_empty() || named.contains(&t.agent_key.as_str()))
        .map(|target| {
            if named.is_empty() && !target.detect_path.exists() {
                AgentOutcome {
                    name: target.name,
                    detected: false,
                    files: Vec::new(),
                    warnings: Vec::new(),
                    errors: Vec::new(),
                }
            } else {
                connect_agent(target, &binary)
            }
        })
        .collect();
    journal::end();
    outcomes
}

fn connect_agent(target: &EditorTarget, binary: &str) -> AgentOutcome {
    let _ = journal::take();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    if let Err(e) = crate::core::editor_registry::write_config(target, binary) {
        errors.push(e);
    }
    match crate::hooks::install_agent_hook(&target.agent_key) {
        Ok(Some(warning)) => warnings.push(warning),
        Ok(None) => {}
        Err(e) => errors.push(e),
    }
    AgentOutcome {
        name: target.name,
        detected: true,
        files: journal::take(),
        warnings,
        errors,
    }
}

/// `~/…` for paths under the home directory.
pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The per-agent report `init` prints: what was found, and every file written.
pub fn print_outcomes(outcomes: &[AgentOutcome], dry_run: bool) {
    let home = crate::core::home::resolve_home_dir().unwrap_or_default();
    let width = outcomes.iter().map(|o| o.name.len()).max().unwrap_or(0);
    for outcome in outcomes {
        let status = if !outcome.detected {
            "not installed"
        } else if !outcome.errors.is_empty() {
            "failed"
        } else if outcome.files.is_empty() {
            "already connected"
        } else if dry_run {
            "would connect"
        } else {
            "connected"
        };
        println!("  {:<width$}  {status}", outcome.name);
        for (path, change) in &outcome.files {
            let verb = change_word(*change, dry_run);
            println!("  {:<width$}    {verb} {}", "", display_path(path, &home));
        }
        for warning in &outcome.warnings {
            println!("  {:<width$}    note: {warning}", "");
        }
        for error in &outcome.errors {
            println!("  {:<width$}    error: {error}", "");
        }
    }
    if !outcomes.iter().any(|o| o.detected) {
        println!("  No agent found. Connect one by name: sobre init <agent>");
    }
}

/// Runs `init`: connects agents, wires the proxy when it is on, and returns
/// the report `--json` prints. `success` is false when any agent failed.
pub fn run_init(named: &[&str], dry_run: bool) -> SetupReport {
    let started_at = chrono::Utc::now();
    if !dry_run && let Ok(dir) = crate::core::data_dir::sobre_data_dir() {
        // The data dir `doctor` checks for.
        let _ = std::fs::create_dir_all(dir);
    }

    let outcomes = connect_agents(named, dry_run);
    if !crate::core::runtime_flags::quiet_enabled() {
        print_outcomes(&outcomes, dry_run);
        if dry_run {
            println!("\n  Run without --check to apply.");
        } else if outcomes.iter().any(|o| !o.files.is_empty()) {
            println!(
                "\n  Restart the agents above so they load sobre, then check with: sobre doctor"
            );
        }
    }
    let home = crate::core::home::resolve_home_dir().unwrap_or_default();
    let mut agents_step = SetupStepReport {
        name: "agents".to_string(),
        ok: outcomes.iter().all(|o| o.errors.is_empty()),
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    for outcome in &outcomes {
        let status = if !outcome.detected {
            "not_detected"
        } else if !outcome.errors.is_empty() {
            "error"
        } else if outcome.files.is_empty() {
            "already"
        } else if dry_run {
            "would_connect"
        } else {
            "connected"
        };
        let files: Vec<String> = outcome
            .files
            .iter()
            .map(|(p, c)| format!("{} {}", change_word(*c, dry_run), display_path(p, &home)))
            .collect();
        agents_step.items.push(SetupItem {
            name: outcome.name.to_string(),
            status: status.to_string(),
            path: None,
            note: (!files.is_empty()).then(|| files.join(", ")),
        });
        agents_step.warnings.extend(
            outcome
                .warnings
                .iter()
                .map(|w| format!("{}: {w}", outcome.name)),
        );
        agents_step.errors.extend(
            outcome
                .errors
                .iter()
                .map(|e| format!("{}: {e}", outcome.name)),
        );
    }

    let mut steps = vec![agents_step];
    if !dry_run {
        steps.push(proxy_step(&home));
    }
    let success = steps.iter().all(|s| s.ok);
    SetupReport {
        schema_version: 1,
        started_at,
        finished_at: chrono::Utc::now(),
        success,
        dry_run,
        platform: PlatformInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        },
        steps,
        warnings: Vec::new(),
        errors: Vec::new(),
    }
}

fn change_word(change: Change, dry_run: bool) -> &'static str {
    match (change, dry_run) {
        (Change::Created, false) => "created",
        (Change::Updated, false) => "updated",
        (Change::Created, true) => "would create",
        (Change::Updated, true) => "would update",
    }
}

/// With the proxy on, points the newly connected agents at it.
fn proxy_step(home: &Path) -> SetupStepReport {
    let mut step = SetupStepReport {
        name: "proxy".to_string(),
        ok: true,
        items: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    if crate::core::config::Config::load().proxy.enabled == Some(true) {
        let port = crate::proxy_setup::default_port();
        crate::proxy_setup::install_proxy_env(home, port, true);
        step.items.push(SetupItem {
            name: "proxy".to_string(),
            status: "configured".to_string(),
            path: None,
            note: Some(format!("agents routed through port {port}")),
        });
    } else {
        step.items.push(SetupItem {
            name: "proxy".to_string(),
            status: "off".to_string(),
            path: None,
            note: Some("turn it on with `sobre proxy enable`".to_string()),
        });
    }
    step
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `doctor --fix` repairs the agents that carry something of sobre's, and
    /// leaves an installed agent sobre was never connected to alone.
    #[test]
    fn agents_with_sobre_are_the_ones_it_was_connected_to() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let write = |rel: &str, content: &str| {
            let path = home.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        let _home = EnvVarGuard::set("HOME", home.to_str().unwrap());
        let _claude = EnvVarGuard::set("CLAUDE_CONFIG_DIR", home.join(".claude").to_str().unwrap());
        let _codex = EnvVarGuard::set("CODEX_HOME", home.join(".codex").to_str().unwrap());
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", home.join(".config").to_str().unwrap());

        // Installed, never connected.
        write(
            ".cursor/mcp.json",
            r#"{"mcpServers": {"other": {"command": "x"}}}"#,
        );
        // An MCP entry.
        write(
            ".claude/.claude.json",
            r#"{"mcpServers": {"sobre": {"command": "sobre"}}}"#,
        );
        write(
            ".codex/config.toml",
            "[mcp_servers.sobre]\ncommand = \"sobre\"\n",
        );
        // Hooks only: the MCP entry was lost.
        write(
            ".gemini/settings.json",
            r#"{"hooks": {"BeforeTool": [{"hooks": [{"command": "/x/sobre hook gemini"}]}]}}"#,
        );

        let mut found = agents_with_sobre(home);
        found.sort();
        assert_eq!(found, ["claude", "codex", "gemini"]);
    }
}
