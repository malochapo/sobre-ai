use std::io::Read;

use super::HOOK_STDIN_TIMEOUT;

const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "svg", "pdf", "zip", "tar", "gz", "bz2",
    "xz", "7z", "rar", "woff", "woff2", "ttf", "otf", "eot", "mp3", "mp4", "wav", "avi", "mov",
    "mkv", "so", "dylib", "dll", "exe", "bin", "o", "a", "class", "pyc", "wasm",
];

/// Handle the `lean-ctx hook deny` subcommand.
///
/// Called by PreToolUse hooks in Replace mode. Denies native Read/Grep/Glob/Shell
/// calls unless an exception applies (binary files, MCP down, etc.).
///
/// Output format matches both Claude Code and Cursor hook protocols.
pub fn handle_deny() {
    let stdin_payload = read_stdin_with_timeout();
    let tool_name = extract_tool_name(&stdin_payload);
    let file_path = extract_file_path(&stdin_payload);

    // #805: deny Write/Edit payloads that contain compression markers.
    // These indicate the agent is writing compressed ctx_read output to disk.
    if is_write_tool(&tool_name) {
        if let Some(content) = extract_write_content(&stdin_payload)
            && has_compression_markers(&content)
        {
            print_deny_compression_markers(&tool_name);
        }
        print_allow();
        return;
    }

    if should_allow(&tool_name, file_path.as_deref()) {
        print_allow();
    } else {
        print_smart_deny(&tool_name, &stdin_payload);
    }
}

fn should_allow(tool_name: &str, file_path: Option<&str>) -> bool {
    if super::is_disabled() {
        return true;
    }

    if file_path.is_some_and(is_binary_file) {
        return true;
    }

    let _ = tool_name;
    false
}

fn is_binary_file(path: &str) -> bool {
    if let Some(ext) = path.rsplit('.').next() {
        return BINARY_EXTENSIONS.contains(&ext.to_lowercase().as_str());
    }
    false
}

fn extract_tool_name(payload: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
        if let Some(name) = json.get("tool_name").and_then(serde_json::Value::as_str) {
            return name.to_string();
        }
        if let Some(name) = json
            .get("hookSpecificInput")
            .and_then(|h| h.get("toolName"))
            .and_then(serde_json::Value::as_str)
        {
            return name.to_string();
        }
    }
    "unknown".to_string()
}

fn extract_file_path(payload: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(payload).ok()?;

    let input = json
        .get("input")
        .or_else(|| json.get("hookSpecificInput").and_then(|h| h.get("input")));

    if let Some(input) = input {
        for key in ["file_path", "path", "filePath"] {
            if let Some(path) = input.get(key).and_then(serde_json::Value::as_str) {
                return Some(path.to_string());
            }
        }
    }
    None
}

fn is_write_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Write"
            | "write"
            | "WriteFile"
            | "Edit"
            | "edit"
            | "MultiEdit"
            | "StrReplace"
            | "str_replace"
            | "EditNotebook"
    )
}

fn has_compression_markers(content: &str) -> bool {
    content.contains("[lean-ctx:")
}

fn extract_write_content(payload: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(payload).ok()?;
    let input = json
        .get("input")
        .or_else(|| json.get("hookSpecificInput").and_then(|h| h.get("input")))?;

    // Check all common content field names across tool variants
    for key in [
        "content",
        "contents",
        "file_text",
        "text",
        "new_string",
        "new_text",
    ] {
        if let Some(text) = input.get(key).and_then(serde_json::Value::as_str) {
            return Some(text.to_string());
        }
    }
    // MultiEdit: check edits array for old_text/new_text
    if let Some(edits) = input.get("edits").and_then(|v| v.as_array()) {
        let mut combined = String::new();
        for edit in edits {
            if let Some(t) = edit.get("new_text").and_then(serde_json::Value::as_str) {
                combined.push_str(t);
            }
            if let Some(t) = edit.get("newText").and_then(serde_json::Value::as_str) {
                combined.push_str(t);
            }
        }
        if !combined.is_empty() {
            return Some(combined);
        }
    }
    None
}

fn print_deny_compression_markers(tool_name: &str) {
    let msg = format!(
        "Blocked {tool_name}: payload contains lean-ctx compression markers \
         ([lean-ctx: omitted ...] or similar). Writing compressed ctx_read \
         output to disk corrupts files. Use ctx_read(raw=true) or ctx_expand \
         to recover full content before editing."
    );
    let output = serde_json::json!({
        // Grok PreToolUse decision field.
        "decision": "deny",
        "reason": msg,
        "permission": "deny",
        "user_message": msg
    });
    println!("{output}");
    std::process::exit(2);
}

/// Build a smart deny message that includes the exact ctx_* call with mapped arguments.
/// This reduces cognitive load for the LLM and prevents instruction drift.
fn smart_deny_message(tool_name: &str, payload: &str) -> String {
    let args = extract_tool_args(payload);
    match tool_name {
        "Read" | "read" | "ReadFile" | "read_file" => build_ctx_read_hint(&args),
        "Grep" | "grep" | "Search" => build_ctx_search_hint(&args),
        "Glob" | "glob" | "list_dir" => build_ctx_glob_hint(&args),
        "Shell" | "Bash" | "bash" | "run_terminal_command" => build_ctx_shell_hint(&args),
        _ => "Use the equivalent ctx_* tool — lean-ctx replace mode is active.".to_string(),
    }
}

fn extract_tool_args(payload: &str) -> serde_json::Map<String, serde_json::Value> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) else {
        return serde_json::Map::new();
    };
    json.get("input")
        .or_else(|| json.get("hookSpecificInput").and_then(|h| h.get("input")))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
}

fn build_ctx_read_hint(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut parts = Vec::new();
    if let Some(path) = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .and_then(serde_json::Value::as_str)
    {
        parts.push(format!("path=\"{path}\""));
    }
    if let Some(start) = args
        .get("offset")
        .or_else(|| args.get("start_line"))
        .and_then(serde_json::Value::as_i64)
    {
        parts.push(format!("start_line={start}"));
    }
    if let Some(limit) = args
        .get("limit")
        .or_else(|| args.get("end_line"))
        .and_then(serde_json::Value::as_i64)
    {
        parts.push(format!("limit={limit}"));
    }
    let call = if parts.is_empty() {
        "ctx_read(path=\"<file>\")".to_string()
    } else {
        format!("ctx_read({})", parts.join(", "))
    };
    format!("[DENIED] Native Read blocked. Use: {call} — lean-ctx replace mode is active.")
}

fn build_ctx_search_hint(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut parts = Vec::new();
    if let Some(pat) = args
        .get("pattern")
        .or_else(|| args.get("regex"))
        .and_then(serde_json::Value::as_str)
    {
        parts.push(format!("pattern=\"{pat}\""));
    }
    if let Some(path) = args
        .get("path")
        .or_else(|| args.get("include"))
        .and_then(serde_json::Value::as_str)
    {
        parts.push(format!("path=\"{path}\""));
    }
    if let Some(glob) = args.get("glob").and_then(serde_json::Value::as_str) {
        parts.push(format!("include=\"{glob}\""));
    }
    let call = if parts.is_empty() {
        "ctx_search(pattern=\"<pattern>\")".to_string()
    } else {
        format!("ctx_search({})", parts.join(", "))
    };
    format!(
        "[DENIED] Native Grep blocked. Use: {call} — ctx_search also supports action=symbol, action=semantic."
    )
}

fn build_ctx_glob_hint(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut parts = Vec::new();
    if let Some(pat) = args
        .get("pattern")
        .or_else(|| args.get("glob_pattern"))
        .and_then(serde_json::Value::as_str)
    {
        parts.push(format!("pattern=\"{pat}\""));
    }
    if let Some(path) = args
        .get("path")
        .or_else(|| args.get("target_directory"))
        .and_then(serde_json::Value::as_str)
    {
        parts.push(format!("path=\"{path}\""));
    }
    let call = if parts.is_empty() {
        "ctx_glob(pattern=\"<glob>\")".to_string()
    } else {
        format!("ctx_glob({})", parts.join(", "))
    };
    format!("[DENIED] Native Glob blocked. Use: {call} — or ctx_tree for directory overview.")
}

fn build_ctx_shell_hint(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let cmd = args
        .get("command")
        .or_else(|| args.get("cmd"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<command>");
    let short_cmd = if cmd.len() > 80 { &cmd[..80] } else { cmd };
    format!(
        "[DENIED] Native Shell blocked. Use: ctx_shell(command=\"{short_cmd}\") — lean-ctx replace mode is active."
    )
}

fn print_smart_deny(tool_name: &str, payload: &str) {
    let msg = smart_deny_message(tool_name, payload);
    let output = serde_json::json!({
        // Grok PreToolUse decision field.
        "decision": "deny",
        "reason": msg,
        "permission": "deny",
        "user_message": msg
    });
    println!("{output}");
    std::process::exit(2);
}

fn print_allow() {
    println!("{{}}");
}

fn read_stdin_with_timeout() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    rx.recv_timeout(HOOK_STDIN_TIMEOUT).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_write_tool_recognizes_all_variants() {
        assert!(is_write_tool("Write"));
        assert!(is_write_tool("write"));
        assert!(is_write_tool("Edit"));
        assert!(is_write_tool("StrReplace"));
        assert!(is_write_tool("MultiEdit"));
        assert!(is_write_tool("EditNotebook"));
        assert!(!is_write_tool("Read"));
        assert!(!is_write_tool("Grep"));
        assert!(!is_write_tool("Shell"));
    }

    #[test]
    fn has_compression_markers_detects_lean_ctx_patterns() {
        assert!(has_compression_markers(
            "some text [lean-ctx: omitted 42 lines] more"
        ));
        assert!(has_compression_markers("... [lean-ctx: archived] ..."));
        assert!(!has_compression_markers("[lean-ctx compressed] tail"));
        assert!(!has_compression_markers(
            "[lean-ctx docs](https://example.com)"
        ));
        assert!(!has_compression_markers(
            "normal file content without markers"
        ));
        assert!(!has_compression_markers("lean-ctx is great"));
        assert!(!has_compression_markers(""));
    }

    #[test]
    fn extract_write_content_from_cursor_write() {
        let payload = r#"{"hookSpecificInput":{"toolName":"Write","input":{"path":"test.md","contents":"hello [lean-ctx: omitted 5 lines]"}}}"#;
        let content = extract_write_content(payload).unwrap();
        assert!(content.contains("[lean-ctx:"));
    }

    #[test]
    fn extract_write_content_from_claude_code_edit() {
        let payload = r#"{"tool_name":"Edit","input":{"path":"test.rs","new_text":"fn foo() { [lean-ctx: omitted 10 lines] }"}}"#;
        let content = extract_write_content(payload).unwrap();
        assert!(content.contains("[lean-ctx:"));
    }

    #[test]
    fn extract_write_content_from_multi_edit() {
        let payload = r#"{"tool_name":"MultiEdit","input":{"path":"x.rs","edits":[{"new_text":"[lean-ctx: omitted 3 lines]"}]}}"#;
        let content = extract_write_content(payload).unwrap();
        assert!(content.contains("[lean-ctx:"));
    }

    #[test]
    fn extract_write_content_clean_payload_returns_none_for_markers() {
        let payload =
            r#"{"tool_name":"Write","input":{"path":"test.md","contents":"normal content"}}"#;
        let content = extract_write_content(payload).unwrap();
        assert!(!has_compression_markers(&content));
    }

    #[test]
    fn extract_write_content_no_content_returns_none() {
        let payload = r#"{"tool_name":"Write","input":{"path":"test.md"}}"#;
        assert!(extract_write_content(payload).is_none());
    }
}
