use super::super::{
    install_mcp_json_agent, mcp_server_quiet_mode, resolve_hook_command_binary, write_file,
};

pub(crate) fn install_windsurf() {
    let home = crate::core::home::resolve_home_dir().unwrap_or_default();
    let config_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    install_mcp_json_agent(
        "Windsurf",
        "~/.codeium/windsurf/mcp_config.json",
        &config_path,
    );
    install_windsurf_hooks(&home);
}

pub(crate) fn install_windsurf_hooks(home: &std::path::Path) {
    let hooks_json = home.join(".codeium").join("windsurf").join("hooks.json");
    let binary = resolve_hook_command_binary();
    let observe_cmd = format!("{binary} hook observe");
    let rewrite_cmd = format!("{binary} hook rewrite");
    let redirect_cmd = format!("{binary} hook redirect");

    let existing_content = if hooks_json.exists() {
        std::fs::read_to_string(&hooks_json).unwrap_or_default()
    } else {
        String::new()
    };

    let mut root = if existing_content.trim().is_empty() {
        serde_json::json!({})
    } else {
        crate::core::jsonc::parse_jsonc(&existing_content).unwrap_or_else(|_| serde_json::json!({}))
    };

    if !root.is_object() {
        root = serde_json::json!({});
    }

    let Some(root_obj) = root.as_object_mut() else {
        return;
    };

    let hooks = root_obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };

    ensure_windsurf_hook_entry(hooks_obj, "pre_mcp_tool_use", &rewrite_cmd, "hook rewrite");
    ensure_windsurf_hook_entry(
        hooks_obj,
        "pre_mcp_tool_use",
        &redirect_cmd,
        "hook redirect",
    );

    let observe_events = [
        "post_mcp_tool_use",
        "post_run_command",
        "post_cascade_response",
        "pre_user_prompt",
    ];

    for event in observe_events {
        ensure_windsurf_hook_entry(hooks_obj, event, &observe_cmd, "hook observe");
    }

    let formatted = serde_json::to_string_pretty(&root).unwrap_or_default();
    let _ = std::fs::create_dir_all(hooks_json.parent().unwrap_or(home));
    write_file(&hooks_json, &formatted);

    if !mcp_server_quiet_mode() {
        eprintln!("Installed Windsurf hooks at {}", hooks_json.display());
    }
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
    let already = entries.iter().any(|e| {
        e.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains(marker))
    });
    if !already {
        entries.push(serde_json::json!({ "command": command }));
    }
}
