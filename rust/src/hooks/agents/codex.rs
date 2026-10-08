use super::super::{CodexHooksFlag, ensure_codex_hooks_enabled, resolve_hook_command_binary};

/// Codex observe hooks in `hooks.json`, and the `[features] hooks` flag Codex
/// needs to run them. There is no shell hook: Codex only accepts a rewritten
/// command together with an `allow` decision, which sobre never emits.
/// `Ok(Some(note))` when the user turned Codex hooks off: the hooks are in
/// place but will not run, and that choice stays theirs.
pub fn install_codex_hook() -> Result<Option<String>, String> {
    let codex_dir = crate::core::home::resolve_codex_dir()
        .ok_or_else(|| "cannot resolve the Codex directory".to_string())?;
    let observe_cmd = format!("{} hook observe", resolve_hook_command_binary());
    crate::config_io::edit_json(&codex_dir.join("hooks.json"), |root| {
        ensure_codex_observe_hooks(root, &observe_cmd);
    })?;

    let config_path = crate::core::home::resolve_codex_config_path()
        .unwrap_or_else(|| codex_dir.join("config.toml"));
    let content = std::fs::read_to_string(&config_path).unwrap_or_default();
    match ensure_codex_hooks_enabled(&content).map_err(|e| {
        format!(
            "{} is not valid TOML ({e}); left unchanged",
            config_path.display()
        )
    })? {
        CodexHooksFlag::Added(updated) => {
            crate::config_io::write_atomic_with_backup(&config_path, &updated).map(|()| None)
        }
        CodexHooksFlag::On => Ok(None),
        CodexHooksFlag::Off => Ok(Some(format!(
            "hooks are turned off in {} ([features] hooks = false), so sobre's observe hooks will not run",
            crate::setup::display_path(
                &config_path,
                &crate::core::home::resolve_home_dir().unwrap_or_default()
            )
        ))),
    }
}

fn ensure_codex_observe_hooks(
    root: &mut serde_json::Map<String, serde_json::Value>,
    observe_cmd: &str,
) {
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };
    for event in ["PostToolUse", "SessionStart", "SessionEnd"] {
        let arr = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| serde_json::json!([]));
        let Some(entries) = arr.as_array_mut() else {
            continue;
        };
        let already = entries.iter().any(|e| {
            e.get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("hook observe"))
                    })
                })
        });
        if !already {
            entries.push(serde_json::json!({
                "matcher": ".*",
                "hooks": [{ "type": "command", "command": observe_cmd, "timeout": 5 }]
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_codex_observe_hooks;

    #[test]
    fn observe_hooks_are_added_once_next_to_the_users() {
        let mut root = serde_json::json!({
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "mine" }] }] }
        });
        let obj = root.as_object_mut().unwrap();
        ensure_codex_observe_hooks(obj, "sobre hook observe");
        ensure_codex_observe_hooks(obj, "sobre hook observe");
        assert_eq!(root["hooks"]["Stop"][0]["hooks"][0]["command"], "mine");
        for event in ["PostToolUse", "SessionStart", "SessionEnd"] {
            assert_eq!(root["hooks"][event].as_array().unwrap().len(), 1, "{event}");
        }
    }
}
