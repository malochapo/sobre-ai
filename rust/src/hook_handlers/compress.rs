//! `hook compress`: shrink a shell command's output after the host ran it.
//!
//! The command itself is never touched: the host decided it may run and ran
//! it, exactly as the agent wrote it. Only the copy of the output the model
//! sees is replaced, and only when compression actually saves tokens. Every
//! replaced output ends with a `[sobre: …]` line that says so and where the
//! full output was kept.
//!
//! One handler, three payload shapes:
//!
//! - **Claude Code** `PostToolUse`: `tool_response` is the Bash result object
//!   (`stdout`, `stderr`, `interrupted`, `isImage`); the reply is
//!   `hookSpecificOutput.updatedToolOutput` with the same shape.
//! - **Copilot CLI** `postToolUse`: `toolResult.textResultForLlm` (or the
//!   VS Code-compatible `tool_result.text_result_for_llm`); the reply is
//!   `modifiedResult`.
//! - **OpenCode**: the sobre plugin's `tool.execute.after` sends `output`
//!   and reads back `output`.
//!
//! Anything else, any parse error, or no saving → no stdout, which every host
//! reads as "keep the original result".

use serde_json::{Value, json};

use super::{HOOK_STDIN_TIMEOUT, is_shell_tool, payload, read_stdin_with_timeout};
use crate::core::tokens::count_tokens;

pub fn handle_compress() {
    let Some(input) = read_stdin_with_timeout(HOOK_STDIN_TIMEOUT) else {
        return;
    };
    if let Some(out) = compute_compress(&input) {
        print!("{out}");
    }
}

pub(super) fn compute_compress(input: &str) -> Option<String> {
    let v: Value = serde_json::from_str(input.trim_start_matches('\u{feff}')).ok()?;
    let tool = payload::resolve_tool_name(&v)?;
    if !is_shell_tool(&tool) {
        return None;
    }
    let args = payload::resolve_tool_args(&v);
    let cmd = payload::resolve_command(&v, args.as_ref())?;

    if let Some(response) = v.get("tool_response") {
        return claude_reply(&cmd, response);
    }
    if let Some(result) = v.get("toolResult") {
        return copilot_reply(&cmd, result, "resultType", "textResultForLlm");
    }
    if let Some(result) = v.get("tool_result") {
        return copilot_reply(&cmd, result, "result_type", "text_result_for_llm");
    }
    let text = v.get("output").and_then(Value::as_str)?;
    let (out, before, after) = compress_text(&cmd, text)?;
    Some(json!({ "output": with_label(&out, &cmd, text, before, after) }).to_string())
}

fn claude_reply(cmd: &str, response: &Value) -> Option<String> {
    let obj = response.as_object()?;
    if obj.get("interrupted").and_then(Value::as_bool) == Some(true)
        || obj.get("isImage").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let stdout = obj.get("stdout").and_then(Value::as_str).unwrap_or("");
    let stderr = obj.get("stderr").and_then(Value::as_str).unwrap_or("");
    let out = compress_text(cmd, stdout);
    let err = compress_text(cmd, stderr);
    if out.is_none() && err.is_none() {
        return None;
    }

    let before = count_tokens(stdout) + count_tokens(stderr);
    let new_stdout = out
        .as_ref()
        .map_or_else(|| stdout.to_string(), |o| o.0.clone());
    let new_stderr = err
        .as_ref()
        .map_or_else(|| stderr.to_string(), |e| e.0.clone());
    let after = count_tokens(&new_stdout) + count_tokens(&new_stderr);
    let full = crate::shell::combine_streams(stdout, stderr, 0);
    let label = label(cmd, &full, before, after);
    let (new_stdout, new_stderr) = if new_stdout.trim().is_empty() {
        (new_stdout, format!("{}\n{label}", new_stderr.trim_end()))
    } else {
        (format!("{}\n{label}", new_stdout.trim_end()), new_stderr)
    };

    let mut updated = obj.clone();
    updated.insert("stdout".into(), Value::String(new_stdout));
    updated.insert("stderr".into(), Value::String(new_stderr));
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedToolOutput": Value::Object(updated)
            }
        })
        .to_string(),
    )
}

fn copilot_reply(cmd: &str, result: &Value, type_key: &str, text_key: &str) -> Option<String> {
    if result.get(type_key).and_then(Value::as_str) != Some("success") {
        return None;
    }
    let text = result.get(text_key).and_then(Value::as_str)?;
    let (out, before, after) = compress_text(cmd, text)?;
    Some(
        json!({
            "modifiedResult": {
                "resultType": "success",
                "textResultForLlm": with_label(&out, cmd, text, before, after)
            }
        })
        .to_string(),
    )
}

/// Compress one stream of a successful command. `None` when there is nothing
/// to gain, or for file reads (`cat`/`head`/`tail`), whose content the agent
/// may edit against and must see verbatim.
fn compress_text(cmd: &str, text: &str) -> Option<(String, usize, usize)> {
    if text.trim().is_empty() || crate::rewrite_registry::is_file_read_command(cmd) {
        return None;
    }
    let compressed = crate::shell::compress::engine::compress_for_outcome(cmd, text, 0);
    let before = count_tokens(text);
    let after = count_tokens(&compressed);
    if after >= before {
        return None;
    }
    let compressed = crate::core::redaction::redact_text(&compressed);
    crate::core::usage::record_compression(
        crate::core::usage::Surface::Hook,
        crate::core::tool_lifecycle::program_name(cmd),
        before,
        after,
    );
    Some((compressed, before, after))
}

fn with_label(out: &str, cmd: &str, original: &str, before: usize, after: usize) -> String {
    format!(
        "{}\n{}",
        out.trim_end(),
        label(cmd, original, before, after)
    )
}

/// The line every replaced output ends with. The full output is kept (redacted,
/// 24h) so nothing compression dropped is out of reach.
fn label(cmd: &str, original: &str, before: usize, after: usize) -> String {
    let kept = crate::shell::save_tee(cmd, original)
        .map(|p| format!("; full output: {p}"))
        .unwrap_or_default();
    format!("[sobre: output compressed, {before} → {after} tokens{kept}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long `git log`, which the git compressor always shortens.
    fn long_output() -> String {
        (0..120)
            .map(|i| {
                format!(
                    "commit {i:040x}\nAuthor: Dev <dev@example.com>\nDate:   Mon Jan 1 10:00:{:02} 2026 +0000\n\n    change number {i}\n\n",
                    i % 60
                )
            })
            .collect::<Vec<_>>()
            .concat()
    }

    #[test]
    fn claude_bash_output_is_replaced_with_same_shape_and_label() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let input = json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "git log" },
            "tool_response": {
                "stdout": long_output(),
                "stderr": "",
                "interrupted": false,
                "isImage": false
            }
        })
        .to_string();
        let out = compute_compress(&input).expect("long build output compresses");
        let v: Value = serde_json::from_str(&out).unwrap();
        let updated = &v["hookSpecificOutput"]["updatedToolOutput"];
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(updated["interrupted"], false);
        assert_eq!(updated["isImage"], false);
        let stdout = updated["stdout"].as_str().unwrap();
        assert!(
            stdout.contains("[sobre: output compressed"),
            "got: {stdout}"
        );
        assert!(stdout.len() < long_output().len());
    }

    /// The kept full output holds stdout and stderr as two blocks, the last
    /// stdout line not glued to the first stderr line.
    #[test]
    fn the_kept_full_output_separates_stdout_from_stderr() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let stdout = long_output();
        let stdout = stdout.trim_end();
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git log" },
            "tool_response": {
                "stdout": stdout,
                "stderr": "warning: refname 'main' is ambiguous.",
                "interrupted": false,
                "isImage": false
            }
        })
        .to_string();
        let out = compute_compress(&input).expect("compresses");
        let v: Value = serde_json::from_str(&out).unwrap();
        let replaced = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        let path = replaced
            .rsplit("full output: ")
            .next()
            .and_then(|rest| rest.strip_suffix(']'))
            .expect("label names the kept output");
        let kept = std::fs::read_to_string(path).unwrap();
        assert!(
            kept.ends_with("change number 119\nwarning: refname 'main' is ambiguous."),
            "{}",
            &kept[kept.len().saturating_sub(120)..]
        );
    }

    #[test]
    fn copilot_result_is_replaced() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let input = json!({
            "toolName": "bash",
            "toolArgs": "{\"command\":\"git log\"}",
            "toolResult": { "resultType": "success", "textResultForLlm": long_output() }
        })
        .to_string();
        let out = compute_compress(&input).expect("compresses");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["modifiedResult"]["resultType"], "success");
        assert!(
            v["modifiedResult"]["textResultForLlm"]
                .as_str()
                .unwrap()
                .contains("[sobre: output compressed")
        );
    }

    #[test]
    fn opencode_plugin_output_is_replaced() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let input = json!({
            "tool_name": "bash",
            "tool_input": { "command": "git log" },
            "output": long_output()
        })
        .to_string();
        let out = compute_compress(&input).expect("compresses");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["output"].as_str().unwrap().contains("[sobre:"));
    }

    #[test]
    fn small_output_file_reads_and_other_tools_are_left_alone() {
        let small = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git status" },
            "tool_response": { "stdout": "clean\n", "stderr": "", "interrupted": false, "isImage": false }
        });
        assert_eq!(compute_compress(&small.to_string()), None);

        let read = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "cat src/main.rs" },
            "tool_response": { "stdout": long_output(), "stderr": "", "interrupted": false, "isImage": false }
        });
        assert_eq!(compute_compress(&read.to_string()), None);

        let other = json!({
            "tool_name": "Read",
            "tool_input": { "file_path": "/x" },
            "tool_response": { "stdout": long_output() }
        });
        assert_eq!(compute_compress(&other.to_string()), None);

        let failed = json!({
            "toolName": "bash",
            "toolArgs": { "command": "git log" },
            "toolResult": { "resultType": "failure", "textResultForLlm": long_output() }
        });
        assert_eq!(compute_compress(&failed.to_string()), None);
    }

    #[test]
    fn interrupted_commands_are_left_alone() {
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git log" },
            "tool_response": { "stdout": long_output(), "stderr": "", "interrupted": true, "isImage": false }
        });
        assert_eq!(compute_compress(&input.to_string()), None);
    }
}
