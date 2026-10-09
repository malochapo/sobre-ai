//! Removes what `init` wrote into the agents' configs: sobre's MCP entries,
//! hooks, skill and plugin. The containers sobre's entries leave empty go too,
//! and a file that held nothing else is deleted. Everything else stays as the
//! user wrote it.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::shorten;

/// What uninstall does to one config file.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Cleanup {
    /// Nothing of sobre's is in it.
    Unchanged,
    /// sobre's entries are gone; the rest of the file, as written.
    Rewrite(String),
    /// Only sobre's entries (and format boilerplate) were in it.
    Delete,
}

/// Keys hosts or `init` add to an otherwise empty file (`{"version": 1}`).
const BOILERPLATE: [&str; 2] = ["version", "$schema"];

/// A JSON config `init` writes into, and what of sobre's may be in it.
struct JsonConfig {
    agent: &'static str,
    path: PathBuf,
    /// The object holding MCP servers, when sobre's entry may be there.
    mcp_container: Option<&'static str>,
    /// Whether sobre's hooks may be in the file's `hooks` object.
    hooks: bool,
}

fn json_configs(home: &Path) -> Vec<JsonConfig> {
    let mut configs: Vec<JsonConfig> = Vec::new();
    for target in crate::core::editor_registry::build_targets(home) {
        let container = match target.config_type {
            crate::core::editor_registry::ConfigType::Codex => continue,
            crate::core::editor_registry::ConfigType::Zed => "context_servers",
            crate::core::editor_registry::ConfigType::VsCodeMcp => "servers",
            crate::core::editor_registry::ConfigType::OpenCode => "mcp",
            _ => "mcpServers",
        };
        configs.push(JsonConfig {
            agent: target.name,
            path: target.config_path,
            mcp_container: Some(container),
            hooks: false,
        });
    }
    let codex_dir = crate::core::home::resolve_codex_dir().unwrap_or_else(|| home.join(".codex"));
    for (agent, path) in [
        (
            "Claude Code",
            crate::core::editor_registry::claude_state_dir(home).join("settings.json"),
        ),
        ("Cursor", home.join(".cursor/hooks.json")),
        ("Codex CLI", codex_dir.join("hooks.json")),
        ("Windsurf", home.join(".codeium/windsurf/hooks.json")),
        (
            "Copilot",
            crate::hooks::agents::copilot_hooks_dir(home).join("hooks.json"),
        ),
        ("Gemini CLI", home.join(".gemini/settings.json")),
    ] {
        match configs.iter_mut().find(|c| c.path == path) {
            Some(existing) => existing.hooks = true,
            None => configs.push(JsonConfig {
                agent,
                path,
                mcp_container: None,
                hooks: true,
            }),
        }
    }
    configs
}

/// Removes sobre from every agent config, skill and plugin. Returns true when
/// anything was removed.
pub(super) fn remove_agent_files(home: &Path) -> bool {
    let mut removed = false;
    try_claude_mcp_remove();

    for config in json_configs(home) {
        let Ok(content) = std::fs::read_to_string(&config.path) else {
            continue;
        };
        let backup = read_backup(&config.path);
        let result = clean_json_text(&content, backup.as_deref(), |root| {
            if let Some(container) = config.mcp_container {
                remove_mcp_entry(root, container);
            }
            if config.hooks {
                remove_sobre_hooks(root);
            }
        });
        removed |= apply(config.agent, &config.path, result, home);
    }

    let codex_config = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| home.join(".codex/config.toml"));
    if let Ok(content) = std::fs::read_to_string(&codex_config) {
        let result = clean_codex_toml(&content, read_backup(&codex_config).as_deref());
        removed |= apply("Codex CLI", &codex_config, result, home);
    }

    let plugin = home.join(".config/opencode/plugins/sobre.js");
    if plugin.exists() {
        removed |= apply("OpenCode", &plugin, Ok(Cleanup::Delete), home);
    }

    for dir in crate::setup::skill::installed_dirs(home) {
        removed = true;
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            println!("  ✗ Could not delete {}: {e}", shorten(&dir, home));
        } else {
            println!("  ✓ Deleted the skill {}", shorten(&dir, home));
            remove_dir_if_empty(dir.parent());
        }
    }
    removed
}

/// The copy of `path` `init` kept from before sobre first edited it. An
/// empty copy means sobre created the file: there is nothing to restore.
fn read_backup(path: &Path) -> Option<String> {
    std::fs::read_to_string(crate::config_io::backup_path_for(path))
        .ok()
        .filter(|backup| !backup.is_empty())
}

/// Writes or deletes `path` as `result` says, and drops the copy `init` kept
/// of the file as it was before sobre.
fn apply(agent: &str, path: &Path, result: Result<Cleanup, String>, home: &Path) -> bool {
    let short = shorten(path, home);
    let done = match result {
        Ok(Cleanup::Unchanged) => false,
        Ok(Cleanup::Rewrite(content)) => {
            if let Err(e) = crate::config_io::write_atomic(path, &content) {
                println!("  ✗ Could not update {short}: {e}");
                return false;
            }
            println!("  ✓ Removed sobre from {short} ({agent})");
            true
        }
        Ok(Cleanup::Delete) => {
            if let Err(e) = std::fs::remove_file(path) {
                println!("  ✗ Could not delete {short}: {e}");
                return false;
            }
            println!("  ✓ Deleted {short} ({agent}: nothing but sobre in it)");
            true
        }
        Err(e) => {
            println!("  ✗ {short} could not be read as config ({e}); left untouched");
            return false;
        }
    };
    let _ = std::fs::remove_file(crate::config_io::backup_path_for(path));
    if !path.exists() {
        remove_dir_if_empty(path.parent());
    }
    done
}

/// Removes a directory sobre created (`plugins/`, `skills/`, `hooks/`) once
/// nothing is left in it.
fn remove_dir_if_empty(dir: Option<&Path>) {
    if let Some(dir) = dir
        && dir
            .file_name()
            .is_some_and(|n| matches!(n.to_str(), Some("plugins" | "skills" | "hooks")))
    {
        let _ = std::fs::remove_dir(dir);
    }
}

/// Applies `edit` to the JSON(C) `content` and says what uninstall should do
/// with the file. Objects and arrays `edit` empties are removed with it.
///
/// `backup` is the file as it was before sobre first edited it. When removing
/// sobre's entries gives back exactly what it held, the backup's own text is
/// restored, so the file is byte for byte what the user had; a file that had
/// a backup existed before sobre and is never deleted.
pub(super) fn clean_json_text(
    content: &str,
    backup: Option<&str>,
    edit: impl FnOnce(&mut Map<String, Value>),
) -> Result<Cleanup, String> {
    let parsed = crate::core::jsonc::parse_jsonc(content).map_err(|e| e.to_string())?;
    let Value::Object(before) = parsed else {
        return Ok(Cleanup::Unchanged);
    };
    let mut unpruned = before.clone();
    edit(&mut unpruned);
    if unpruned == before {
        return Ok(Cleanup::Unchanged);
    }
    let mut pruned = unpruned.clone();
    prune_emptied(&before, &mut pruned);

    if let Some(backup) = backup {
        if let Ok(Value::Object(original)) = crate::core::jsonc::parse_jsonc(backup)
            && (original == pruned || original == unpruned)
        {
            return Ok(Cleanup::Rewrite(backup.to_string()));
        }
    } else if pruned.keys().all(|k| BOILERPLATE.contains(&k.as_str())) {
        return Ok(Cleanup::Delete);
    }
    Ok(Cleanup::Rewrite(crate::core::jsonc::rewrite_preserving(
        content,
        &Value::Object(pruned),
    )))
}

/// Drops the objects and arrays that were non-empty in `before` and are empty
/// in `after`: the containers that held only what was removed.
fn prune_emptied(before: &Map<String, Value>, after: &mut Map<String, Value>) {
    after.retain(|key, value| match (before.get(key), value) {
        (Some(Value::Object(old)), Value::Object(new)) => {
            prune_emptied(old, new);
            new.is_empty() == old.is_empty()
        }
        (Some(Value::Array(old)), Value::Array(new)) => new.is_empty() == old.is_empty(),
        _ => true,
    });
}

fn remove_mcp_entry(root: &mut Map<String, Value>, container: &str) {
    if let Some(servers) = root.get_mut(container).and_then(Value::as_object_mut) {
        servers.remove("sobre");
    }
}

/// True for a hook command sobre wrote: `<binary> hook <action>`.
fn is_sobre_command(command: &str) -> bool {
    command.contains("sobre") && command.contains(" hook ")
}

/// True for a hook entry whose command (`command`, Copilot's `bash` and
/// `powershell`) is one of sobre's.
fn is_sobre_hook(entry: &Value) -> bool {
    ["command", "bash", "powershell"].iter().any(|k| {
        entry
            .get(*k)
            .and_then(Value::as_str)
            .is_some_and(is_sobre_command)
    })
}

/// Removes sobre's hooks from the `hooks` object, in each host's shape: flat
/// entries (Cursor, Windsurf, Copilot) and matcher groups holding a `hooks`
/// list (Claude Code, Codex, Gemini). A group left without hooks goes too.
fn remove_sobre_hooks(root: &mut Map<String, Value>) {
    let Some(events) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    for entries in events.values_mut().filter_map(Value::as_array_mut) {
        entries.retain_mut(|entry| {
            if is_sobre_hook(entry) {
                return false;
            }
            match entry.get_mut("hooks").and_then(Value::as_array_mut) {
                Some(group) => {
                    let had_hooks = !group.is_empty();
                    group.retain(|h| !is_sobre_hook(h));
                    !(had_hooks && group.is_empty())
                }
                None => true,
            }
        });
    }
}

/// Removes `[mcp_servers.sobre]` (and `[mcp_servers]` if it held only that)
/// and the hooks flag `init` added from Codex's `config.toml`. Like
/// [`clean_json_text`], restores `backup` verbatim when that is what is left.
fn clean_codex_toml(content: &str, backup: Option<&str>) -> Result<Cleanup, String> {
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    let mut changed = false;
    if let Some(servers) = doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        changed |= servers.remove("sobre").is_some();
        if changed && servers.is_empty() {
            doc.remove("mcp_servers");
        }
    }
    let mut text = doc.to_string();
    if let Some(without_flag) = crate::hooks::remove_codex_hooks_flag(&text) {
        text = without_flag;
        changed = true;
    }
    if !changed {
        return Ok(Cleanup::Unchanged);
    }
    match backup {
        Some(backup)
            if toml::from_str::<toml::Value>(backup).ok()
                == toml::from_str::<toml::Value>(&text).ok() =>
        {
            Ok(Cleanup::Rewrite(backup.to_string()))
        }
        None if text.trim().is_empty() => Ok(Cleanup::Delete),
        _ => Ok(Cleanup::Rewrite(text)),
    }
}

/// Claude Code rewrites `~/.claude.json` while it runs; its CLI removes the
/// entry in a way it keeps. The file is cleaned afterwards either way.
fn try_claude_mcp_remove() {
    let _ = std::process::Command::new("claude")
        .args(["mcp", "remove", "sobre", "--scope", "user"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean_mcp(content: &str, container: &str) -> Cleanup {
        clean_json_text(content, None, |root| remove_mcp_entry(root, container)).unwrap()
    }

    fn clean_hooks(content: &str) -> Cleanup {
        clean_json_text(content, None, remove_sobre_hooks).unwrap()
    }

    #[test]
    fn mcp_entry_goes_and_the_rest_keeps_its_text() {
        let input = "{\n  // mine\n  \"theme\": \"x\",\n  \"mcpServers\": {\n    \"other\": { \"command\": \"o\" },\n    \"sobre\": { \"command\": \"/bin/sobre\" }\n  }\n}\n";
        let Cleanup::Rewrite(out) = clean_mcp(input, "mcpServers") else {
            panic!("expected a rewrite");
        };
        assert_eq!(
            out,
            "{\n  // mine\n  \"theme\": \"x\",\n  \"mcpServers\": {\n    \"other\": { \"command\": \"o\" }\n  }\n}\n"
        );
    }

    #[test]
    fn a_container_left_empty_goes_and_an_only_sobre_file_is_deleted() {
        let input = "{\n  \"theme\": \"x\",\n  \"context_servers\": { \"sobre\": { \"command\": \"s\" } }\n}\n";
        assert_eq!(
            clean_mcp(input, "context_servers"),
            Cleanup::Rewrite("{\n  \"theme\": \"x\"\n}\n".to_string())
        );
        assert_eq!(
            clean_mcp(
                r#"{ "mcpServers": { "sobre": { "command": "s" } } }"#,
                "mcpServers"
            ),
            Cleanup::Delete
        );
    }

    #[test]
    fn a_users_empty_container_is_left_alone() {
        let input = r#"{ "mcpServers": {}, "x": 1 }"#;
        assert_eq!(clean_mcp(input, "mcpServers"), Cleanup::Unchanged);
    }

    #[test]
    fn nested_hook_groups_keep_the_users_hooks() {
        let input = r#"{
  "permissions": { "allow": ["Bash(ls)"] },
  "hooks": {
    "PostToolUse": [
      { "matcher": "Bash", "hooks": [
        { "type": "command", "command": "/bin/sobre hook compress" },
        { "type": "command", "command": "my-check" }
      ] },
      { "matcher": ".*", "hooks": [{ "type": "command", "command": "/bin/sobre hook observe" }] }
    ],
    "Stop": [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "/bin/sobre hook observe" }] }]
  }
}"#;
        let Cleanup::Rewrite(out) = clean_hooks(input) else {
            panic!("expected a rewrite");
        };
        let v = crate::core::jsonc::parse_jsonc(&out).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls)");
        let post = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(post[0]["hooks"][0]["command"], "my-check");
        assert!(v["hooks"].get("Stop").is_none(), "{out}");
    }

    #[test]
    fn flat_and_copilot_hooks_files_holding_only_sobre_are_deleted() {
        let cursor = r#"{ "version": 1, "hooks": { "preToolUse": [{ "matcher": "Shell", "command": "/bin/sobre hook cursor" }] } }"#;
        assert_eq!(clean_hooks(cursor), Cleanup::Delete);
        let copilot = r#"{ "version": 1, "hooks": { "postToolUse": [{ "type": "command", "bash": "\"/bin/sobre\" hook compress", "powershell": "& \"/bin/sobre\" hook compress" }] } }"#;
        assert_eq!(clean_hooks(copilot), Cleanup::Delete);
    }

    #[test]
    fn a_hooks_file_with_user_hooks_is_kept() {
        let input = r#"{ "version": 1, "hooks": { "preToolUse": [
            { "matcher": "Shell", "command": "/bin/sobre hook cursor" },
            { "matcher": "Shell", "command": "my-own" }
        ] } }"#;
        let Cleanup::Rewrite(out) = clean_hooks(input) else {
            panic!("expected a rewrite");
        };
        assert!(out.contains("my-own") && !out.contains("sobre"), "{out}");
    }

    #[test]
    fn unparseable_json_is_an_error_and_files_without_sobre_are_unchanged() {
        assert!(clean_json_text("{ broken", None, remove_sobre_hooks).is_err());
        assert_eq!(
            clean_hooks(r#"{ "hooks": { "Stop": [{ "command": "mine" }] } }"#),
            Cleanup::Unchanged
        );
    }

    #[test]
    fn codex_config_loses_only_what_init_added() {
        let user = "model = \"o3\"\n\n[features]\nweb_search = true\n";
        let crate::hooks::CodexHooksFlag::Added(with_flag) =
            crate::hooks::ensure_codex_hooks_enabled(user).unwrap()
        else {
            panic!("flag added");
        };
        let installed =
            crate::core::editor_registry::upsert_codex_toml(&with_flag, "/bin/sobre").unwrap();
        assert_eq!(
            clean_codex_toml(&installed, None).unwrap(),
            Cleanup::Rewrite(user.to_string())
        );

        let only_sobre = crate::core::editor_registry::upsert_codex_toml("", "/bin/sobre").unwrap();
        assert_eq!(
            clean_codex_toml(&only_sobre, None).unwrap(),
            Cleanup::Delete
        );

        let users_flag = "[features]\nhooks = true\n";
        assert_eq!(
            clean_codex_toml(users_flag, None).unwrap(),
            Cleanup::Unchanged
        );
    }

    #[test]
    fn the_backup_text_comes_back_when_nothing_else_changed() {
        let original = "{ \"theme\": \"GitHub\" }\n";
        let installed = "{\n  \"theme\": \"GitHub\",\n  \"mcpServers\": { \"sobre\": { \"command\": \"s\" } }\n}\n";
        let result = clean_json_text(installed, Some(original), |root| {
            remove_mcp_entry(root, "mcpServers");
        })
        .unwrap();
        assert_eq!(result, Cleanup::Rewrite(original.to_string()));

        // The user's empty container from before sobre comes back too.
        let had_empty = "{ \"mcpServers\": {} }";
        let installed = "{ \"mcpServers\": { \"sobre\": { \"command\": \"s\" } } }";
        let result = clean_json_text(installed, Some(had_empty), |root| {
            remove_mcp_entry(root, "mcpServers");
        })
        .unwrap();
        assert_eq!(result, Cleanup::Rewrite(had_empty.to_string()));
    }

    #[test]
    fn a_file_that_existed_before_sobre_is_never_deleted() {
        let user_changed_since = "{ \"other\": 1 }";
        let installed = "{ \"mcpServers\": { \"sobre\": { \"command\": \"s\" } } }";
        let Cleanup::Rewrite(out) = clean_json_text(installed, Some(user_changed_since), |root| {
            remove_mcp_entry(root, "mcpServers");
        })
        .unwrap() else {
            panic!("expected a rewrite");
        };
        assert_eq!(
            crate::core::jsonc::parse_jsonc(&out).unwrap(),
            serde_json::json!({})
        );
    }
}
