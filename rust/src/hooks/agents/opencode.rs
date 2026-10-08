use super::super::{mcp_server_quiet_mode, resolve_binary_path, write_file};

pub(crate) fn install_opencode_hook() {
    let binary = resolve_binary_path();
    let home = crate::core::home::resolve_home_dir().unwrap_or_default();
    install_opencode_plugin(&home, &binary);
    let config_path = crate::core::editor_registry::opencode_config_path(&home);
    let display_path = config_path.display().to_string();

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let desired = serde_json::json!({
        "type": "local",
        "command": [&binary],
        "enabled": true
    });

    let file_existed = config_path.exists();
    let content = if file_existed {
        std::fs::read_to_string(&config_path).unwrap_or_default()
    } else {
        String::new()
    };
    let has_sobre = content.contains("sobre");

    let mut json = crate::core::jsonc::parse_jsonc(&content)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    let Some(obj) = json.as_object_mut() else {
        return;
    };

    let mcp_written = if has_sobre {
        false
    } else {
        if !file_existed {
            obj.insert(
                "$schema".to_string(),
                serde_json::json!("https://opencode.ai/config.json"),
            );
        }
        let mcp = obj.entry("mcp").or_insert_with(|| serde_json::json!({}));
        if let Some(mcp_obj) = mcp.as_object_mut() {
            mcp_obj.insert("sobre".to_string(), desired);
            true
        } else {
            false
        }
    };

    if mcp_written {
        if file_existed {
            let backup = config_path.with_extension("json.bak");
            let _ = std::fs::copy(&config_path, &backup);
        }
        if let Ok(formatted) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(&config_path, formatted);
            if !mcp_server_quiet_mode() {
                eprintln!("  \x1b[32m✓\x1b[0m OpenCode MCP configured at {display_path}");
            }
        }
    } else if has_sobre && !mcp_server_quiet_mode() {
        eprintln!("OpenCode MCP already configured at {display_path}");
    }
}

/// `~/.config/opencode/plugins/sobre.js`: after the `bash` tool ran and
/// succeeded, hand its output to `hook compress` and keep the reply when there
/// is one. The command itself is never changed.
pub(crate) fn install_opencode_plugin(home: &std::path::Path, binary: &str) {
    let path = home.join(".config/opencode/plugins/sobre.js");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_file(&path, &opencode_plugin_source(binary));
}

fn opencode_plugin_source(binary: &str) -> String {
    let bin = serde_json::to_string(binary).unwrap_or_else(|_| "\"sobre\"".to_string());
    format!(
        r#"// Installed by sobre. Compresses the output of a successful `bash` tool
// call after it ran; the command itself is never changed. Compressed output
// ends with a "[sobre: ...]" line saying so.
import {{ spawnSync }} from "node:child_process";

const BIN = {bin};

export const Sobre = async () => ({{
  "tool.execute.after": async (input, output) => {{
    if (input.tool !== "bash" || output?.metadata?.exit !== 0) return;
    if (typeof output.output !== "string" || output.output.length === 0) return;
    const payload = JSON.stringify({{
      tool_name: "bash",
      tool_input: {{ command: input.args?.command ?? "" }},
      output: output.output,
    }});
    const r = spawnSync(BIN, ["hook", "compress"], {{
      input: payload,
      encoding: "utf8",
      timeout: 10000,
    }});
    if (r.status !== 0 || !r.stdout) return;
    try {{
      const reply = JSON.parse(r.stdout);
      if (typeof reply.output === "string") output.output = reply.output;
    }} catch {{}}
  }},
}});
"#
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn plugin_embeds_the_quoted_binary() {
        let src = super::opencode_plugin_source("/opt/my dir/sobre");
        assert!(src.contains(r#"const BIN = "/opt/my dir/sobre";"#));
        assert!(src.contains(r#"["hook", "compress"]"#));
    }
}
