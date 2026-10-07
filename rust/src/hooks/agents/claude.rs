use super::super::{mcp_server_quiet_mode, resolve_hook_command_binary, write_file};

pub(crate) fn install_claude_hook() {
    let Some(home) = crate::core::home::resolve_home_dir() else {
        tracing::error!("Cannot resolve home directory");
        return;
    };

    install_claude_hook_config(&home);
    install_claude_mcp_server(&home);
}

fn install_claude_mcp_server(home: &std::path::Path) {
    let config_path = crate::core::editor_registry::claude_mcp_json_path(home);
    let binary = super::super::resolve_binary_path();

    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    if existing.contains("\"lean-ctx\"") && existing.contains("mcpServers") {
        return;
    }

    let parsed: Result<serde_json::Value, _> = if existing.trim().is_empty() {
        Ok(serde_json::json!({}))
    } else {
        crate::core::jsonc::parse_jsonc(&existing)
    };

    if let Ok(mut root) = parsed
        && let Some(obj) = root.as_object_mut()
    {
        let servers = obj
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(servers_obj) = servers.as_object_mut()
            && !servers_obj.contains_key("lean-ctx")
        {
            servers_obj.insert(
                "lean-ctx".to_string(),
                serde_json::json!({
                    "command": binary,
                    "args": []
                }),
            );
            write_file(
                &config_path,
                &serde_json::to_string_pretty(&root).unwrap_or_default(),
            );
            if !super::super::mcp_server_quiet_mode() {
                eprintln!("Added lean-ctx MCP server to {}", config_path.display());
            }
        }
    }
}

/// PostToolUse matcher for the guard-safe re-read dedup (GL #1140): Read only —
/// the handler mirrors the Read result shape and must never see another tool's.
const READ_DEDUP_MATCHER: &str = "Read";

/// Ensure the PostToolUse `hook read-dedup` entry exists exactly once (GL #1140).
///
/// Runs alongside the observe hook on PostToolUse; read-dedup (Read) and
/// compress (Bash) are the only `updatedToolOutput` writers and never see the
/// same tool, so there is no competing-rewriter race. The handler is
/// inert off guard hosts and for first reads (`read_dedup = auto`), so
/// installing it unconditionally is safe.
fn ensure_read_dedup_hook(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    read_dedup_cmd: &str,
) {
    let post = hooks_obj
        .entry("PostToolUse".to_string())
        .or_insert_with(|| serde_json::json!([]));
    if let Some(post_arr) = post.as_array_mut() {
        ensure_command_hook(post_arr, READ_DEDUP_MATCHER, read_dedup_cmd);
    }
}

/// The trailing action token of a lean-ctx hook command, e.g. `"hook compress"`.
///
/// Commands are rendered as `<binary> hook <action>`, where `<binary>` is either a bare
/// `lean-ctx` (when it is on `PATH`) or an absolute path (when it is not). The action token
/// is therefore the only path-independent way to recognise a lean-ctx hook.
fn lean_ctx_action_token(command: &str) -> &str {
    match command.rfind(" hook ") {
        Some(i) => command[i + 1..].trim_end(),
        None => command.trim_end(),
    }
}

/// True if `hook` is a lean-ctx command hook for `action`, regardless of how the binary path
/// was rendered (bare `lean-ctx` vs an absolute path).
fn is_lean_ctx_command_for(hook: &serde_json::Value, action: &str) -> bool {
    if hook.get("type").and_then(|t| t.as_str()) != Some("command") {
        return false;
    }
    let Some(cmd) = hook.get("command").and_then(|c| c.as_str()) else {
        return false;
    };
    if !cmd.contains("lean-ctx") {
        return false;
    }
    cmd.trim_end().ends_with(action)
}

/// Ensure exactly one lean-ctx hook for `command`'s action exists under `matcher`.
///
/// Earlier versions deduped on the *full* command string, but `resolve_binary_path()` renders
/// the binary as a bare `lean-ctx` (on `PATH`) or an absolute path (off `PATH`), so the
/// rendering flips between `install` and `update` and the exact-string compare missed the
/// existing hook — appending a duplicate on every run. We now strip every stale lean-ctx hook
/// for this action (any path, any matcher group) and re-add a
/// single fresh one. This is idempotent across re-runs *and* self-heals settings files already
/// duplicated by older versions, while leaving any non-lean-ctx hooks untouched.
fn ensure_command_hook(pre_arr: &mut Vec<serde_json::Value>, matcher: &str, command: &str) {
    let action = lean_ctx_action_token(command);

    for group in pre_arr.iter_mut() {
        if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            hooks.retain(|h| !is_lean_ctx_command_for(h, action));
        }
    }
    pre_arr.retain(|g| {
        g.get("hooks")
            .and_then(|h| h.as_array())
            .is_none_or(|hooks| !hooks.is_empty())
    });

    let desired = serde_json::json!({ "type": "command", "command": command });
    if let Some(group) = pre_arr
        .iter_mut()
        .find(|g| g.get("matcher").and_then(|m| m.as_str()) == Some(matcher))
    {
        if let Some(obj) = group.as_object_mut() {
            match obj
                .entry("hooks".to_string())
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
            {
                Some(hooks) => hooks.insert(0, desired),
                None => {
                    obj.insert("hooks".to_string(), serde_json::json!([desired]));
                }
            }
        }
        return;
    }
    pre_arr.push(serde_json::json!({ "matcher": matcher, "hooks": [desired] }));
}

fn ensure_claude_observe_hooks(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    observe_cmd: &str,
) {
    let observe_hook = serde_json::json!([{
        "matcher": ".*",
        "hooks": [{ "type": "command", "command": observe_cmd }]
    }]);

    let observe_events = [
        "PostToolUse",
        "UserPromptSubmit",
        "Stop",
        "PreCompact",
        "SessionStart",
        "SessionEnd",
    ];

    for event in observe_events {
        let entry = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| serde_json::json!([]));

        if let Some(arr) = entry.as_array() {
            let already = arr.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .is_some_and(|hooks| {
                        hooks.iter().any(|hook| {
                            hook.get("command")
                                .and_then(|c| c.as_str())
                                .is_some_and(|c| c.contains("hook observe"))
                        })
                    })
            });
            if already {
                continue;
            }
        }

        if let Some(arr) = entry.as_array_mut() {
            arr.push(serde_json::json!({
                "matcher": ".*",
                "hooks": [{ "type": "command", "command": observe_cmd }]
            }));
        } else {
            *entry = observe_hook.clone();
        }
    }
}

pub(crate) fn install_claude_hook_config(home: &std::path::Path) {
    let binary = resolve_hook_command_binary();
    let compress_cmd = format!("{binary} hook compress");
    let observe_cmd = format!("{binary} hook observe");
    let read_dedup_cmd = format!("{binary} hook read-dedup");

    let settings_path = crate::core::editor_registry::claude_state_dir(home).join("settings.json");
    let settings_content = std::fs::read_to_string(&settings_path).unwrap_or_default();
    let mut root = if settings_content.trim().is_empty() {
        serde_json::json!({})
    } else if let Ok(parsed) = crate::core::jsonc::parse_jsonc(&settings_content) {
        parsed
    } else {
        return;
    };
    let before = serde_json::to_string_pretty(&root).unwrap_or_default();

    if let Some(root_obj) = root.as_object_mut() {
        let hooks = root_obj
            .entry("hooks".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(hooks_obj) = hooks.as_object_mut() {
            // Observe first: `ensure_command_hook` re-appends its entry, so
            // this order is the one a re-run reproduces (idempotent file).
            ensure_claude_observe_hooks(hooks_obj, &observe_cmd);
            // Shell output is compressed after the command ran (PostToolUse
            // `updatedToolOutput`); the command itself is never rewritten.
            let post = hooks_obj
                .entry("PostToolUse".to_string())
                .or_insert_with(|| serde_json::json!([]));
            if let Some(post_arr) = post.as_array_mut() {
                ensure_command_hook(post_arr, BASH_MATCHER, &compress_cmd);
            }
            ensure_read_dedup_hook(hooks_obj, &read_dedup_cmd);
        }
    }

    // Only rewrite (with backup) when the merge actually changed something, so a
    // no-op `update` doesn't churn the user's settings.json or pile up backups.
    let after = serde_json::to_string_pretty(&root).unwrap_or_default();
    if after != before {
        write_file(&settings_path, &after);
    }
    if !mcp_server_quiet_mode() {
        eprintln!("Installed Claude Code hooks in {}", settings_path.display());
    }
}

/// The Claude Code shell tools whose output `hook compress` may shrink.
const BASH_MATCHER: &str = if cfg!(windows) {
    "Bash|PowerShell"
} else {
    "Bash"
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BASH: &str = "Bash";

    fn commands_for(pre: &[serde_json::Value], action: &str) -> Vec<String> {
        pre.iter()
            .filter_map(|g| g.get("hooks").and_then(|h| h.as_array()))
            .flatten()
            .filter(|h| is_lean_ctx_command_for(h, action))
            .filter_map(|h| h.get("command").and_then(|c| c.as_str()).map(String::from))
            .collect()
    }

    #[test]
    fn action_token_is_path_independent() {
        assert_eq!(
            lean_ctx_action_token("lean-ctx hook compress"),
            "hook compress"
        );
        assert_eq!(
            lean_ctx_action_token("/Users/x/.local/bin/lean-ctx hook read-dedup"),
            "hook read-dedup"
        );
    }

    #[test]
    fn path_flip_refreshes_instead_of_duplicating() {
        // Installed once off PATH (absolute path), then `update` runs on PATH (bare name).
        let mut pre = vec![json!({
            "matcher": BASH,
            "hooks": [{ "type": "command", "command": "/abs/bin/lean-ctx hook compress" }]
        })];
        ensure_command_hook(&mut pre, BASH, "lean-ctx hook compress");
        assert_eq!(
            commands_for(&pre, "hook compress"),
            ["lean-ctx hook compress"]
        );
    }

    #[test]
    fn repeated_calls_are_idempotent() {
        let mut pre = vec![];
        for _ in 0..5 {
            ensure_command_hook(&mut pre, BASH, "lean-ctx hook compress");
        }
        assert_eq!(
            commands_for(&pre, "hook compress"),
            ["lean-ctx hook compress"]
        );
    }

    #[test]
    fn heals_preexisting_duplicates_across_groups() {
        // Two copies of the same hook with different matchers/paths.
        let mut pre = vec![
            json!({ "matcher": "Bash", "hooks": [{ "type": "command", "command": "/a/lean-ctx hook compress" }] }),
            json!({ "matcher": BASH, "hooks": [{ "type": "command", "command": "lean-ctx hook compress" }] }),
        ];
        ensure_command_hook(&mut pre, BASH, "lean-ctx hook compress");
        assert_eq!(
            commands_for(&pre, "hook compress"),
            ["lean-ctx hook compress"]
        );
    }

    #[test]
    fn preserves_foreign_hooks() {
        let mut pre = vec![json!({
            "matcher": BASH,
            "hooks": [
                { "type": "command", "command": "my-own-tool --flag" },
                { "type": "command", "command": "/abs/lean-ctx hook compress" }
            ]
        })];
        ensure_command_hook(&mut pre, BASH, "lean-ctx hook compress");
        assert_eq!(
            commands_for(&pre, "hook compress"),
            ["lean-ctx hook compress"]
        );
        let all: Vec<String> = pre
            .iter()
            .filter_map(|g| g.get("hooks").and_then(|h| h.as_array()))
            .flatten()
            .filter_map(|h| h.get("command").and_then(|c| c.as_str()).map(String::from))
            .collect();
        assert!(
            all.iter().any(|c| c == "my-own-tool --flag"),
            "foreign hook dropped: {all:?}"
        );
    }

    #[test]
    fn compress_and_read_dedup_coexist_after_reruns() {
        let mut post = vec![];
        ensure_command_hook(&mut post, BASH, "lean-ctx hook compress");
        ensure_command_hook(&mut post, READ_DEDUP_MATCHER, "lean-ctx hook read-dedup");
        ensure_command_hook(&mut post, BASH, "lean-ctx hook compress"); // re-run (update)
        assert_eq!(
            commands_for(&post, "hook compress"),
            ["lean-ctx hook compress"]
        );
        assert_eq!(
            commands_for(&post, "hook read-dedup"),
            ["lean-ctx hook read-dedup"]
        );
    }

    #[test]
    fn read_dedup_hook_installs_once_alongside_observe() {
        // GL #1140: the PostToolUse array carries observe (matcher .*) AND
        // read-dedup (matcher Read); repeated installs must not duplicate either.
        let mut hooks_obj = serde_json::Map::new();
        ensure_claude_observe_hooks(&mut hooks_obj, "lean-ctx hook observe");
        ensure_read_dedup_hook(&mut hooks_obj, "lean-ctx hook read-dedup");
        ensure_read_dedup_hook(&mut hooks_obj, "lean-ctx hook read-dedup"); // re-run

        let post = hooks_obj
            .get("PostToolUse")
            .and_then(|p| p.as_array())
            .expect("PostToolUse array");
        let dedup_entries: Vec<&serde_json::Value> = post
            .iter()
            .filter(|g| {
                g.get("hooks").and_then(|h| h.as_array()).is_some_and(|h| {
                    h.iter().any(|hook| {
                        hook.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.ends_with("hook read-dedup"))
                    })
                })
            })
            .collect();
        assert_eq!(dedup_entries.len(), 1, "exactly one read-dedup group");
        assert_eq!(
            dedup_entries[0].get("matcher").and_then(|m| m.as_str()),
            Some("Read"),
            "read-dedup must match ONLY the Read tool (shape safety)"
        );
        let observe_present = post.iter().any(|g| {
            g.get("hooks").and_then(|h| h.as_array()).is_some_and(|h| {
                h.iter().any(|hook| {
                    hook.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.contains("hook observe"))
                })
            })
        });
        assert!(
            observe_present,
            "observe hook must survive read-dedup install"
        );
    }
}
