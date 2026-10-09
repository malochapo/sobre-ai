use super::super::resolve_hook_command_binary;

/// PostToolUse matcher for the guard-safe re-read dedup: Read only —
/// the handler mirrors the Read result shape and must never see another tool's.
const READ_DEDUP_MATCHER: &str = "Read";

/// Ensure the PostToolUse `hook read-dedup` entry exists exactly once.
///
/// Runs alongside the observe hook on PostToolUse; read-dedup (Read) and
/// compress (Bash) are the only `updatedToolOutput` writers and never see the
/// same tool, so there is no competing-rewriter race. The handler is
/// inert off guard hosts and for first reads, so installing it
/// unconditionally is safe.
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

/// The trailing action token of a sobre hook command, e.g. `"hook compress"`.
///
/// Commands are rendered as `<binary> hook <action>`, where `<binary>` is either a bare
/// `sobre` (when it is on `PATH`) or an absolute path (when it is not). The action token
/// is therefore the only path-independent way to recognise a sobre hook.
fn sobre_action_token(command: &str) -> &str {
    match command.rfind(" hook ") {
        Some(i) => command[i + 1..].trim_end(),
        None => command.trim_end(),
    }
}

/// True if `hook` is a sobre command hook for `action`, regardless of how the binary path
/// was rendered (bare `sobre` vs an absolute path).
fn is_sobre_command_for(hook: &serde_json::Value, action: &str) -> bool {
    if hook.get("type").and_then(|t| t.as_str()) != Some("command") {
        return false;
    }
    let Some(cmd) = hook.get("command").and_then(|c| c.as_str()) else {
        return false;
    };
    if !crate::hooks::is_sobre_hook_command(cmd) {
        return false;
    }
    cmd.trim_end().ends_with(action)
}

/// Ensure exactly one sobre hook for `command`'s action exists under `matcher`.
///
/// A sobre hook is recognised by its action token, whatever its binary path or matcher
/// group: every one for this action is stripped and a single fresh one re-added, so re-runs
/// stay idempotent when the binary path changes. Non-sobre hooks are left untouched.
fn ensure_command_hook(pre_arr: &mut Vec<serde_json::Value>, matcher: &str, command: &str) {
    let action = sobre_action_token(command);

    for group in pre_arr.iter_mut() {
        if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            hooks.retain(|h| !is_sobre_command_for(h, action));
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

        if let Some(arr) = entry.as_array_mut()
            && super::refresh_commands(arr, "hook observe", observe_cmd)
        {
            continue;
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

pub(crate) fn install_claude_hook_config(home: &std::path::Path) -> Result<(), String> {
    let binary = resolve_hook_command_binary();
    let compress_cmd = format!("{binary} hook compress");
    let observe_cmd = format!("{binary} hook observe");
    let read_dedup_cmd = format!("{binary} hook read-dedup");

    let settings_path = crate::core::editor_registry::claude_state_dir(home).join("settings.json");
    crate::config_io::edit_json(&settings_path, |root| {
        let hooks = root
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
    })
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
            .filter(|h| is_sobre_command_for(h, action))
            .filter_map(|h| h.get("command").and_then(|c| c.as_str()).map(String::from))
            .collect()
    }

    #[test]
    fn action_token_is_path_independent() {
        assert_eq!(sobre_action_token("sobre hook compress"), "hook compress");
        assert_eq!(
            sobre_action_token("/Users/x/.local/bin/sobre hook read-dedup"),
            "hook read-dedup"
        );
    }

    #[test]
    fn path_flip_refreshes_instead_of_duplicating() {
        // Installed once off PATH (absolute path), then re-run on PATH (bare name).
        let mut pre = vec![json!({
            "matcher": BASH,
            "hooks": [{ "type": "command", "command": "/abs/bin/sobre hook compress" }]
        })];
        ensure_command_hook(&mut pre, BASH, "sobre hook compress");
        assert_eq!(commands_for(&pre, "hook compress"), ["sobre hook compress"]);
    }

    #[test]
    fn repeated_calls_are_idempotent() {
        let mut pre = vec![];
        for _ in 0..5 {
            ensure_command_hook(&mut pre, BASH, "sobre hook compress");
        }
        assert_eq!(commands_for(&pre, "hook compress"), ["sobre hook compress"]);
    }

    #[test]
    fn heals_preexisting_duplicates_across_groups() {
        // Two copies of the same hook with different matchers/paths.
        let mut pre = vec![
            json!({ "matcher": "Bash", "hooks": [{ "type": "command", "command": "/a/sobre hook compress" }] }),
            json!({ "matcher": BASH, "hooks": [{ "type": "command", "command": "sobre hook compress" }] }),
        ];
        ensure_command_hook(&mut pre, BASH, "sobre hook compress");
        assert_eq!(commands_for(&pre, "hook compress"), ["sobre hook compress"]);
    }

    #[test]
    fn preserves_foreign_hooks() {
        let mut pre = vec![json!({
            "matcher": BASH,
            "hooks": [
                { "type": "command", "command": "my-own-tool --flag" },
                { "type": "command", "command": "/abs/sobre hook compress" }
            ]
        })];
        ensure_command_hook(&mut pre, BASH, "sobre hook compress");
        assert_eq!(commands_for(&pre, "hook compress"), ["sobre hook compress"]);
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
        ensure_command_hook(&mut post, BASH, "sobre hook compress");
        ensure_command_hook(&mut post, READ_DEDUP_MATCHER, "sobre hook read-dedup");
        ensure_command_hook(&mut post, BASH, "sobre hook compress"); // re-run
        assert_eq!(
            commands_for(&post, "hook compress"),
            ["sobre hook compress"]
        );
        assert_eq!(
            commands_for(&post, "hook read-dedup"),
            ["sobre hook read-dedup"]
        );
    }

    #[test]
    fn read_dedup_hook_installs_once_alongside_observe() {
        // The PostToolUse array carries observe (matcher .*) AND
        // read-dedup (matcher Read); repeated installs must not duplicate either.
        let mut hooks_obj = serde_json::Map::new();
        ensure_claude_observe_hooks(&mut hooks_obj, "sobre hook observe");
        ensure_read_dedup_hook(&mut hooks_obj, "sobre hook read-dedup");
        ensure_read_dedup_hook(&mut hooks_obj, "sobre hook read-dedup"); // re-run

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
