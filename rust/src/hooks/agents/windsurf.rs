use super::super::resolve_hook_command_binary;

/// Windsurf observe hooks. Windsurf has no hook that can change a tool's
/// output, so sobre only observes there.
pub(crate) fn install_windsurf_hooks(home: &std::path::Path) -> Result<(), String> {
    let hooks_json = home.join(".codeium").join("windsurf").join("hooks.json");
    let observe_cmd = format!("{} hook observe", resolve_hook_command_binary());
    crate::config_io::edit_json(&hooks_json, |root| {
        let hooks = root
            .entry("hooks".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let Some(hooks_obj) = hooks.as_object_mut() else {
            return;
        };
        for event in [
            "post_mcp_tool_use",
            "post_run_command",
            "post_cascade_response",
            "pre_user_prompt",
        ] {
            ensure_windsurf_hook_entry(hooks_obj, event, &observe_cmd, "hook observe");
        }
    })
}

fn ensure_windsurf_hook_entry(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    command: &str,
    marker: &str,
) {
    let arr = hooks_obj
        .entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]));
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let Some(entries) = arr.as_array_mut() else {
        return;
    };
    if !super::refresh_commands(entries, marker, command) {
        entries.push(serde_json::json!({ "command": command }));
    }
}
