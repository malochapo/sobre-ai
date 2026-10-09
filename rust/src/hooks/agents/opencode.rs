/// The OpenCode plugin that compresses `bash` output. The file is sobre's
/// alone; `uninstall` deletes it.
pub(crate) fn install_opencode_plugin(home: &std::path::Path, binary: &str) -> Result<(), String> {
    let path = home.join(".config/opencode/plugins/sobre.js");
    if let Some(parent) = path.parent() {
        crate::config_io::ensure_dir(parent)?;
    }
    crate::config_io::write_owned(&path, &opencode_plugin_source(binary))
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
