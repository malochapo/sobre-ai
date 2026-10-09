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
    let upstream = state.openai_upstream();
    forward::forward_request(
        State(state),
        req,
        &upstream,
        "/v1/chat/completions",
        compress_request_body,
        Provider::OpenAi,
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

    if let Some(messages) = doc.get_mut("messages").and_then(|m| m.as_array_mut()) {
        let tool_names = tool_kind::openai_tool_names(messages);

        // OpenAI's automatic prompt caching is prefix-based like Anthropic's,
        // so history is pruned at the same frozen, cache-aware boundary.
        let boundary = super::history_prune::prune_boundary(messages.len());
        // Mirror the Anthropic guard: never rewrite content behind a client
        // `cache_control` breakpoint. OpenAI requests carry none, so this
        // resolves to 0 and pruning is byte-for-byte unchanged — but the code
        // path stays uniform across providers.
        let cached = super::history_prune::cached_prefix_len(messages);
        modified |=
            super::history_prune::prune_history_range(messages, cached, boundary, &tool_names);

        for msg in messages.iter_mut() {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role != "tool" {
                continue;
            }

            let name = msg
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .and_then(|id| tool_names.get(id))
                .map(String::as_str);
            let kind = name.map_or(ToolResultKind::Other, tool_kind::classify_tool_name);

            // Skip live compression when the tool is excluded.
            if name.is_some_and(|n| cfg.proxy.is_tool_live_compress_excluded(n)) {
                continue;
            }
            if let Some(mut content) = msg
                .get_mut("content")
                .and_then(|c| c.as_str().map(String::from))
                && super::tool_output::compress_text(&mut content, name, kind)
            {
                msg["content"] = Value::String(content);
                modified = true;
            }
        }
    }

    let out = serde_json::to_vec(&doc).unwrap_or_default();
    let compressed_size = if modified { out.len() } else { original_size };
    (out, original_size, compressed_size)
}

#[cfg(test)]
mod tests {
    use super::super::compress::compress_tool_result;
    use super::*;

    #[test]
    fn read_file_tool_result_protected() {
        let code = (0..60)
            .map(|i| format!("    const value{i} = computeValue{i}(ctx, opts);"))
            .collect::<Vec<_>>()
            .join("\n");
        let body = serde_json::json!({
            "model": "gpt-5",
            "messages": [
                {"role": "assistant", "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "read_file"}}]},
                {"role": "tool", "tool_call_id": "call_1", "content": code}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _orig, _comp) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            parsed["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("value59")
        );
    }

    #[test]
    fn json_envelope_tool_output_is_compressed() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let raw = long_git_status();
        let expected = compress_tool_result(&raw, Some("Bash"));
        let envelope = serde_json::to_string(&serde_json::json!({
            "content": [{"type": "text", "text": raw}],
            "isError": false,
        }))
        .unwrap();
        let body = serde_json::json!({
            "model": "gpt-5",
            "messages": [
                {"role": "assistant", "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "Bash"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": envelope}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());

        assert!(comp < orig, "JSON envelope tool output should shrink");
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let content = parsed["messages"][1]["content"].as_str().unwrap();
        let envelope: Value = serde_json::from_str(content).unwrap();
        assert_eq!(envelope["content"][0]["text"].as_str().unwrap(), expected);
    }

    #[test]
    fn streamed_request_is_not_given_stream_options() {
        // The proxy never changes the protocol the client asked for: a
        // streamed request without `stream_options` leaves without one.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({"model": "gpt-5.4", "stream": true, "messages": []});
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, o, c) = compress_request_body(body, bytes.len());
        assert_eq!(c, o);
        assert_eq!(out, bytes, "the request must stay byte-identical");
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

    fn big_prose() -> String {
        let p = "You are a careful, senior software engineer. You always explain your \
                 reasoning before making changes, you prefer small reviewable diffs, and \
                 you never introduce mock data or placeholders into production code. ";
        [p; 6].join("\n")
    }

    #[test]
    fn system_and_assistant_prose_pass_through_verbatim() {
        // Role aggressiveness is off, so prose is never rewritten.
        let _iso = crate::core::data_dir::isolated_data_dir();

        let prose = big_prose();
        let body = serde_json::json!({
            "model": "gpt-5",
            "messages": [
                {"role": "system", "content": prose},
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": prose},
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(
            parsed["messages"][0]["content"].as_str().unwrap(),
            prose,
            "system prose must pass through verbatim"
        );
        assert_eq!(
            parsed["messages"][2]["content"].as_str().unwrap(),
            prose,
            "assistant turns must pass through verbatim"
        );
    }

    #[test]
    fn openai_request_rewrite_is_deterministic() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let prose = big_prose();
        let mk = || {
            serde_json::json!({
                "model": "gpt-5",
                "messages": [{"role": "system", "content": prose}, {"role": "user", "content": "hi"}]
            })
        };
        let (a, b) = (mk(), mk());
        let la = serde_json::to_vec(&a).unwrap().len();
        let lb = serde_json::to_vec(&b).unwrap().len();
        assert_eq!(
            compress_request_body(a, la).0,
            compress_request_body(b, lb).0,
            "identical input must yield byte-identical output"
        );
    }

    #[test]
    fn effort_control_is_off_and_a_noop() {
        // End-to-end through the Chat Completions request path.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "gpt-5.5", "messages": [{"role": "user", "content": "hi"}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();

        // Off: a cache-safe no-op (size unchanged, no param added).
        let (off, o, c) = compress_request_body(body, bytes.len());
        assert_eq!(c, o, "effort off must be a passthrough");
        assert!(
            serde_json::from_slice::<Value>(&off)
                .unwrap()
                .get("reasoning_effort")
                .is_none()
        );
    }
}
