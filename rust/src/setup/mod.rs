//! `sobre init`: connects agents (hooks, and on request the MCP entry and the
//! skill) and reports, per agent, the files it wrote. `init --check` runs the
//! same code with writes turned off.

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

/// A temporary HOME that every agent's files resolve into, with sobre's own
/// directories isolated too. Holds the environment lock; the variables are
/// restored before it is released.
#[cfg(test)]
pub(crate) struct TestHome {
    _env: Vec<EnvVarGuard>,
    _iso: crate::core::data_dir::IsolatedDataDir,
    tmp: tempfile::TempDir,
}

#[cfg(test)]
impl TestHome {
    pub(crate) fn new() -> Self {
        let iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let at = |rel: &str| tmp.path().join(rel).to_str().unwrap().to_string();
        let env = vec![
            EnvVarGuard::set("HOME", tmp.path().to_str().unwrap()),
            EnvVarGuard::set("CLAUDE_CONFIG_DIR", &at(".claude")),
            EnvVarGuard::set("CODEX_HOME", &at(".codex")),
            EnvVarGuard::set("COPILOT_HOME", &at(".copilot")),
            EnvVarGuard::set("XDG_CONFIG_HOME", &at(".config")),
            EnvVarGuard::set("APPDATA", &at("AppData")),
        ];
        Self {
            _env: env,
            _iso: iso,
            tmp,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        self.tmp.path()
    }

    pub(crate) fn write(&self, rel: &str, content: &str) {
        let path = self.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// The command of sobre's entry in a JSON MCP config, if it has one.
    pub(crate) fn mcp_command(&self, rel: &str) -> Option<String> {
        let content = std::fs::read_to_string(self.path().join(rel)).ok()?;
        let config: serde_json::Value = serde_json::from_str(&content).ok()?;
        let command = config["mcpServers"]["sobre"]["command"].as_str()?;
        Some(command.to_string())
    }
}

/// Whether `connect_agents` writes the MCP entry and the skill, besides the
/// hooks. Together they add about 400 tokens to every request, and agents
/// rarely call the tools, so they are opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mcp {
    /// Write them for every agent connected: `init --mcp`, or a yes to its
    /// question.
    Connect,
    /// Refresh them where they already are, and add them nowhere.
    KeepExisting,
}

/// What `init` did for one agent.
#[derive(Debug)]
pub struct AgentOutcome {
    pub name: &'static str,
    /// False when the agent is not installed and was not named: nothing was
    /// written for it.
    pub detected: bool,
    /// True for an agent without hooks (Zed) left without the MCP entry:
    /// nothing was written for it, and that is no failure.
    pub needs_mcp: bool,
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
/// Copilot CLI and VS Code share their hooks file and skill directory: a
/// sobre hook or skill there counts once, as Copilot CLI's, so the repair
/// refreshes the shared hooks once and adds no MCP entry. VS Code counts by
/// its MCP entry alone.
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

/// Whether the hooks, skill or plugin `init` gives `agent` are there; `None`
/// for an agent that has none (Zed) or shares Copilot CLI's (VS Code).
fn own_footprint(agent: &str, home: &Path) -> Option<bool> {
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    let hooks = match agent {
        "claude" => crate::core::editor_registry::claude_state_dir(home).join("settings.json"),
        "copilot" => crate::hooks::agents::copilot_hooks_dir(home).join("hooks.json"),
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

/// Whether an agent `connect_agents(named, …)` would connect lacks sobre's
/// MCP entry: what `init` asks about.
pub fn any_lacks_mcp(named: &[&str]) -> bool {
    let Some(home) = crate::core::home::resolve_home_dir() else {
        return false;
    };
    crate::core::editor_registry::build_targets(&home)
        .iter()
        .filter(|t| {
            if named.is_empty() {
                t.detect_path.exists()
            } else {
                named.contains(&t.agent_key.as_str())
            }
        })
        .any(|t| !has_mcp_entry(t))
}

/// Connects agents: the named ones (installed or not), or else every one that
/// is installed. Each gets its hooks, and the MCP entry and the skill as `mcp`
/// says. With `dry_run` nothing is written and the outcomes say what would be.
pub fn connect_agents(named: &[&str], mcp: Mcp, dry_run: bool) -> Vec<AgentOutcome> {
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
                    needs_mcp: false,
                    files: Vec::new(),
                    warnings: Vec::new(),
                    errors: Vec::new(),
                }
            } else {
                connect_agent(target, &binary, &home, mcp)
            }
        })
        .collect();
    journal::end();
    outcomes
}

fn connect_agent(target: &EditorTarget, binary: &str, home: &Path, mcp: Mcp) -> AgentOutcome {
    let _ = journal::take();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    let agent = target.agent_key.as_str();
    // An entry already there is refreshed, never removed. The skill only
    // documents the MCP tools: it goes where the entry goes, and is refreshed
    // where it already is.
    let with_mcp = mcp == Mcp::Connect || has_mcp_entry(target);
    if with_mcp && let Err(e) = crate::core::editor_registry::write_config(target, binary) {
        errors.push(e);
    }
    if (with_mcp || skill::skill_path(agent, home).is_some_and(|p| p.exists()))
        && let Err(e) = skill::install(agent, home)
    {
        errors.push(e);
    }
    match crate::hooks::install_agent_hook(agent) {
        Ok(Some(warning)) => warnings.push(warning),
        Ok(None) => {}
        Err(e) => errors.push(e),
    }
    let needs_mcp = !with_mcp && !crate::hooks::has_hooks(agent);
    if needs_mcp {
        warnings.push(format!(
            "no hooks to connect; sobre reaches {} through MCP only (sobre init {agent} --mcp)",
            target.name
        ));
    }
    AgentOutcome {
        name: target.name,
        detected: true,
        needs_mcp,
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
        } else if outcome.needs_mcp {
            "not connected"
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
/// the report `--json` prints. `success` is false when any agent failed; an
/// agent that needs the MCP server to be connected at all is no failure.
pub fn run_init(named: &[&str], mcp: Mcp, dry_run: bool) -> SetupReport {
    let started_at = chrono::Utc::now();
    if !dry_run && let Ok(dir) = crate::core::data_dir::sobre_data_dir() {
        // The data dir `doctor` checks for.
        let _ = std::fs::create_dir_all(dir);
    }

    let outcomes = connect_agents(named, mcp, dry_run);
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
        } else if outcome.needs_mcp {
            "needs_mcp"
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
    #[serial_test::serial(claude_config_dir)]
    fn agents_with_sobre_are_the_ones_it_was_connected_to() {
        let home = TestHome::new();

        // Installed, never connected.
        home.write(
            ".cursor/mcp.json",
            r#"{"mcpServers": {"other": {"command": "x"}}}"#,
        );
        // An MCP entry.
        home.write(
            ".claude/.claude.json",
            r#"{"mcpServers": {"sobre": {"command": "sobre"}}}"#,
        );
        home.write(
            ".codex/config.toml",
            "[mcp_servers.sobre]\ncommand = \"sobre\"\n",
        );
        // Hooks only.
        home.write(
            ".gemini/settings.json",
            r#"{"hooks": {"BeforeTool": [{"hooks": [{"command": "/x/sobre hook gemini"}]}]}}"#,
        );
        // The hooks Copilot CLI shares with VS Code: they count once, as
        // Copilot CLI's.
        home.write(
            ".copilot/hooks/hooks.json",
            r#"{"version": 1, "hooks": {"postToolUse": [{"type": "command", "bash": "\"/x/sobre\" hook observe", "powershell": "& \"/x/sobre\" hook observe"}]}}"#,
        );

        let mut found = agents_with_sobre(home.path());
        found.sort();
        assert_eq!(found, ["claude", "codex", "copilot", "gemini"]);
    }

    /// Without `--mcp`, an agent gets its hooks, and the MCP entry and the
    /// skill only where the entry already is; with it, every agent gets them.
    #[test]
    #[serial_test::serial(claude_config_dir)]
    fn mcp_and_skill_come_on_request_or_where_the_entry_is() {
        let home = TestHome::new();
        home.write(
            ".cursor/mcp.json",
            r#"{"mcpServers": {"sobre": {"command": "/old/sobre"}}}"#,
        );
        let claude_json = crate::core::editor_registry::claude_mcp_json_path(home.path());
        let claude_skill = skill::skill_path("claude", home.path()).unwrap();
        assert!(any_lacks_mcp(&["claude", "cursor"]));
        assert!(!any_lacks_mcp(&["cursor"]));

        let outcomes = connect_agents(&["claude", "cursor"], Mcp::KeepExisting, false);
        assert!(
            outcomes.iter().all(|o| o.errors.is_empty() && !o.needs_mcp),
            "{outcomes:?}"
        );
        // Claude Code: the hooks alone.
        assert!(home.path().join(".claude/settings.json").exists());
        assert!(!claude_json.exists());
        assert!(!claude_skill.exists());
        // Cursor: its entry refreshed, the skill written with it.
        let binary = crate::core::portable_binary::resolve_portable_binary();
        assert_eq!(home.mcp_command(".cursor/mcp.json"), Some(binary.clone()));
        assert!(skill::is_current("cursor", home.path()));
        assert!(home.path().join(".cursor/hooks.json").exists());

        // `init --check --mcp` lists the MCP files and writes nothing.
        let preview = connect_agents(&["claude"], Mcp::Connect, true);
        let listed: Vec<&PathBuf> = preview[0].files.iter().map(|(p, _)| p).collect();
        assert!(
            listed.contains(&&claude_json) && listed.contains(&&claude_skill),
            "{listed:?}"
        );
        assert!(!claude_json.exists() && !claude_skill.exists());

        connect_agents(&["claude"], Mcp::Connect, false);
        assert_eq!(
            home.mcp_command(".claude/.claude.json"),
            Some(binary.clone())
        );
        assert!(skill::is_current("claude", home.path()));
        assert!(!any_lacks_mcp(&["claude", "cursor"]));
    }

    /// Zed has no hooks: without the MCP server nothing is written for it,
    /// and that is no failure.
    #[test]
    #[serial_test::serial(claude_config_dir)]
    fn zed_without_mcp_gets_nothing_and_does_not_fail() {
        let home = TestHome::new();
        let settings = crate::core::editor_registry::zed_settings_path(home.path());

        let outcomes = connect_agents(&["zed"], Mcp::KeepExisting, false);
        let zed = &outcomes[0];
        assert!(zed.detected && zed.needs_mcp, "{zed:?}");
        assert!(zed.files.is_empty() && zed.errors.is_empty(), "{zed:?}");
        assert!(zed.warnings[0].contains("sobre init zed --mcp"), "{zed:?}");
        assert!(!settings.exists());

        let outcomes = connect_agents(&["zed"], Mcp::Connect, false);
        let zed = &outcomes[0];
        assert!(!zed.needs_mcp && zed.errors.is_empty(), "{zed:?}");
        assert!(settings.exists());
    }
}
