use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::Value;

use super::ProxyState;
use super::forward;
use super::tool_kind::{self, ToolResultKind};
use super::usage::Provider;

pub(crate) async fn handler(
    State(state): State<ProxyState>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    let upstream = state.anthropic_upstream();
    forward::forward_request(
        State(state),
        req,
        &upstream,
        "/v1/messages",
        compress_request_body,
        Provider::Anthropic,
    )
    .await
}

pub(super) fn compress_request_body(
    parsed: Value,
    original_size: usize,
) -> (Vec<u8>, usize, usize) {
    let mut doc = parsed;
    let mut modified = false;

    let cfg = crate::core::config::Config::load();
    // Length of the client's provider-cached message prefix, which pruning
    // below must not touch.
    let cached = doc
        .get("messages")
        .and_then(|m| m.as_array())
        .map_or(0, |m| super::history_prune::cached_prefix_len(m));

    if let Some(messages) = doc.get_mut("messages").and_then(|m| m.as_array_mut()) {
        // Resolve tool-call id → tool name so file/source reads can be protected
        // from lossy compression that would force the model to re-read mid-task.
        let tool_names = tool_kind::anthropic_tool_names(messages);

        // Prune at a frozen, cache-aware boundary by default: Anthropic's
        // prompt cache matches exact prefixes, so the boundary must not move
        // every turn (see `history_prune::prune_boundary`).
        let boundary = super::history_prune::prune_boundary(messages.len());
        // Never rewrite content the client has marked with `cache_control`:
        // pruning inside the already-cached prefix invalidates Anthropic's
        // prompt cache from the first changed message (#448). Pruning therefore
        // starts after the last breakpoint; with no breakpoint this is 0, i.e.
        // the previous behaviour.
        modified |=
            super::history_prune::prune_history_range(messages, cached, boundary, &tool_names);

        for msg in messages.iter_mut() {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role != "user" {
                continue;
            }

            if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
                for block in content.iter_mut() {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }

                    let name = block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .and_then(|id| tool_names.get(id))
                        .map(String::as_str);
                    let kind = name.map_or(ToolResultKind::Other, tool_kind::classify_tool_name);

                    // #481: skip live compression when the originating tool is
                    // on the exclusion list (Serena).
                    let excluded =
                        name.is_some_and(|n| cfg.proxy.is_tool_live_compress_excluded(n));
                    if !excluded && let Some(inner_content) = block.get_mut("content") {
                        modified |= compress_content_field(inner_content, name, kind);
                    }
                }
            }
        }
    }

    let out = serde_json::to_vec(&doc).unwrap_or_default();
    let compressed_size = if modified { out.len() } else { original_size };
    (out, original_size, compressed_size)
}

/// Compresses a tool_result `content` field unless it is a protected file/source
/// read, which must reach the model intact (it is what gets edited).
fn compress_content_field(
    content: &mut Value,
    tool_name: Option<&str>,
    kind: ToolResultKind,
) -> bool {
    match content {
        Value::String(s) => super::tool_output::compress_text(s, tool_name, kind),
        Value::Array(arr) => {
            let mut modified = false;
            for item in arr.iter_mut() {
                if item.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(Value::String(text)) = item.get_mut("text")
                {
                    modified |= super::tool_output::compress_text(text, tool_name, kind);
                }
            }
            modified
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::compress::compress_tool_result;
    use super::*;

    fn source_file_body() -> Vec<u8> {
        let code = (0..60)
            .map(|i| format!("    let binding_{i} = compute_value_{i}(context, options);"))
            .collect::<Vec<_>>()
            .join("\n");
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [
                {
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": "src/app.rs"}}]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": code}]
                }
            ]
        });
        serde_json::to_vec(&body).unwrap()
    }

    #[test]
    fn read_tool_result_is_never_truncated() {
        let bytes = source_file_body();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let (out, _orig, _comp) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let content = parsed["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            content.contains("binding_59"),
            "the full source body must survive — refactors need it intact"
        );
        assert!(!content.contains("lines omitted"));
    }

    fn forge_log_body(tool_name: &str) -> Value {
        // Generic, highly-repetitive log with no `$ cmd` hint, so routing falls
        // back to the tool name (exercising the foreign-tool classification)
        // and the generic compressor (not a command-specific pattern).
        let mut log = String::new();
        for i in 0..90 {
            log.push_str(&format!(
                "INFO  processing item {i}: ok, latency={i}ms, queue depth normal, retries 0\n"
            ));
        }
        serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "f1", "name": tool_name, "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "f1", "content": log}]}
            ]
        })
    }

    #[test]
    fn forge_shell_tool_result_compresses() {
        // A vendor-prefixed foreign shell tool reaches the proxy; its log output
        // must still be compressed (rtk/ctx_* never see another server's tools).
        let body = forge_log_body("forge_shell");
        let bytes = serde_json::to_vec(&body).unwrap();
        let (_out, orig, comp) = compress_request_body(body, bytes.len());
        assert!(comp < orig, "foreign shell output must be compressed");
    }

    #[test]
    fn foreign_read_tool_protects_source() {
        // `forge_read` is classified FileRead via the segment fallback, so the
        // source body must reach the model intact (it is what gets edited).
        let code = (0..60)
            .map(|i| format!("    let binding_{i} = compute_value_{i}(context, options);"))
            .collect::<Vec<_>>()
            .join("\n");
        let body = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "r1", "name": "forge_read", "input": {"path": "src/app.rs"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "r1", "content": code}]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _orig, _comp) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let content = parsed["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            content.contains("binding_59"),
            "source body must survive intact"
        );
    }

    #[test]
    fn compress_request_body_is_deterministic() {
        // tee path depends on the data dir; serialize env access so a parallel
        // test never swaps SOBRE_DATA_DIR between the two compressions.
        let _lock = crate::core::data_dir::test_env_lock();
        // #498: the proxy rewrite must be a pure function of the body so the
        // provider prompt-cache prefix stays byte-identical across turns.
        let bytes = serde_json::to_vec(&forge_log_body("Bash")).unwrap();
        let a = compress_request_body(serde_json::from_slice(&bytes).unwrap(), bytes.len()).0;
        let b = compress_request_body(serde_json::from_slice(&bytes).unwrap(), bytes.len()).0;
        assert_eq!(a, b, "identical input must yield byte-identical output");
    }

    /// Long, duplicate-rich natural-language prose that compresses cleanly.
    fn big_prose() -> String {
        let p = "You are a careful, senior software engineer. You always explain your \
                 reasoning before making changes, you prefer small reviewable diffs, and \
                 you never introduce mock data or placeholders into production code. ";
        [p; 6].join("\n")
    }

    #[test]
    fn prose_passes_through_verbatim() {
        // #710: role aggressiveness is off, so system, user and assistant prose
        // all reach the provider unchanged.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let prose = big_prose();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": prose,
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": prose}]},
                {"role": "assistant", "content": prose},
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _orig, _comp) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["system"].as_str().unwrap(), prose);
        assert_eq!(
            parsed["messages"][0]["content"][0]["text"]
                .as_str()
                .unwrap(),
            prose
        );
        assert_eq!(parsed["messages"][1]["content"].as_str().unwrap(), prose);
    }

    #[test]
    fn bash_tool_result_still_compresses() {
        let log = {
            let mut s = String::from(
                "$ git status\nOn branch main\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n",
            );
            for i in 0..90 {
                s.push_str(&format!("\tmodified:   src/module_{i}/file_{i}.rs\n"));
            }
            s.push_str("\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n");
            s
        };
        let body = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": log}]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (_out, orig, comp) = compress_request_body(body, bytes.len());
        assert!(comp < orig, "shell output must still be compressed");
    }

    #[test]
    fn json_envelope_tool_result_is_compressed() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let log = long_git_status();
        let expected = compress_tool_result(&log, Some("Bash"));
        let envelope = serde_json::to_string(&serde_json::json!({
            "content": [{"type": "text", "text": log}],
            "isError": false,
        }))
        .unwrap();
        let body = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [{
                    "type": "tool_use",
                    "id": "t1",
                    "name": "Bash",
                    "input": {}
                }]},
                {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": envelope
                }]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());

        assert!(comp < orig, "JSON envelope tool result should shrink");
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let content = parsed["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        let envelope: Value = serde_json::from_str(content).unwrap();
        assert_eq!(envelope["content"][0]["text"].as_str().unwrap(), expected);
    }

    fn long_git_status() -> String {
        let mut s = String::from(
            "$ git status\nOn branch main\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n",
        );
        for i in 0..80 {
            s.push_str(&format!("\tmodified:   src/module_{i}/file_{i}.rs\n"));
        }
        s.push_str("\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n");
        s
    }

    #[test]
    fn effort_control_is_off() {
        // #834: effort control is off, so even a request that asked for
        // adaptive thinking gets no `output_config` added.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let adaptive = serde_json::json!({
            "model": "claude-opus-4-8",
            "thinking": {"type": "adaptive"},
            "messages": [{"role": "user", "content": "hi"}]
        });
        let bytes = serde_json::to_vec(&adaptive).unwrap();
        let (out, _o, _c) = compress_request_body(adaptive, bytes.len());
        assert!(
            serde_json::from_slice::<Value>(&out)
                .unwrap()
                .get("output_config")
                .is_none()
        );
    }

    /// A system prompt comfortably over Anthropic's minimum cacheable size, so
    /// the #939 breakpoint gate fires.
    fn cacheable_system() -> String {
        "You are a careful, senior software engineer who writes maintainable code. ".repeat(400)
    }

    #[test]
    fn cache_breakpoint_off_is_byte_unchanged() {
        // Off → the request must be byte-identical (no system reshape),
        // preserving the meter-only / cache-stable contract.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        assert_eq!(out, bytes, "the request must stay byte-identical");
    }

    #[test]
    fn cache_aligner_measures_without_mutating_body() {
        // #940: the volatile-field scan is telemetry-only — it is on and must leave
        // the request byte-identical (measurement, not a rewrite), even on a
        // volatile-field-rich system prompt.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": "Today is 2026-06-22. Session 550e8400-e29b-41d4-a716-446655440000.",
            "messages": [{"role": "user", "content": "Hello."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        assert_eq!(
            out, bytes,
            "cache-aligner telemetry must never mutate the request body"
        );
    }

    /// A cacheable-size system prompt that carries one volatile field (a date),
    /// so the #974 relocate has something to move and clears the size floor.
    fn cacheable_system_with_date() -> String {
        format!("Today is 2026-06-27. {}", cacheable_system())
    }

    #[test]
    fn volatile_system_prompt_is_never_rewritten() {
        // The cache-aligner only measures: a volatile-rich system prompt
        // leaves the request byte-identical.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system_with_date(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        assert_eq!(out, bytes, "the request must stay byte-identical");
    }
}
