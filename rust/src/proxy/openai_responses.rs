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

/// Proxy handler for OpenAI's Responses API (`POST /v1/responses`).
///
/// The Responses API superseded Chat Completions for clients such as opencode
/// and the OpenAI Agents SDK. Its conversation turns live in `input` rather than
/// `messages`, so the Chat Completions handler never saw — and never compressed —
/// them. This handler reuses the same upstream, auth and streaming path but
/// understands the Responses-API request shape.
///
/// Retrieve / cancel / delete / input_items sub-paths
/// (`/v1/responses/{id}/...`) are routed here as well and pass through untouched:
/// they carry no `input` array, so `compress_request_body` is a no-op for them.
///
/// Handles `POST /v1/responses` (and the bare `/responses`) over HTTP/SSE.
pub(crate) async fn handler(
    State(state): State<ProxyState>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    let upstream = state.openai_upstream();
    forward::forward_request(
        State(state),
        req,
        &upstream,
        "/v1/responses",
        compress_request_body,
        "OpenAI",
        &[],
    )
    .await
}

/// Handles the WebSocket Responses transport on `GET /v1/responses`.
///
/// Codex (and the OpenAI SDK) default to `ws://…/responses` with one
/// `response.create` event per turn. Bridging the upgrade here lets the proxy be
/// a drop-in for Codex without forcing `supports_websockets = false` (#440); the
/// actual WS↔HTTP/SSE bridging lives in `openai_responses_ws`.
pub(crate) async fn ws_handler(
    State(state): State<ProxyState>,
    headers: axum::http::HeaderMap,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    super::openai_responses_ws::upgrade(state, ws, &headers)
}

pub(super) fn compress_request_body(
    parsed: Value,
    original_size: usize,
) -> (Vec<u8>, usize, usize) {
    let mut doc = parsed;
    let mut modified = false;
    // Two-stage, like the Chat Completions path: (1) cache-aware prune of the
    // frozen OLD region — old file reads collapse to re-read stubs, old logs
    // head/tail summarize — then (2) compress whatever recent outputs remain.
    // Stage 1 runs first so a stubbed old output isn't needlessly re-compressed.
    modified |= prune_responses_input(&mut doc);
    modified |= compress_responses_input(&mut doc);
    let out = serde_json::to_vec(&doc).unwrap_or_default();
    let compressed_size = if modified { out.len() } else { original_size };
    (out, original_size, compressed_size)
}

/// Cache-aware history pruning for the Responses API.
///
/// Unlike the Chat Completions path we never *remove* an item: the Responses API
/// rejects a `function_call` whose matching `function_call_output` is absent (and
/// reasoning items must keep their originating call). Instead we rewrite the
/// `output` text of every `function_call_output` in the frozen OLD region
/// (`input[..boundary]`) — pairing and ordering are untouched, so there is no
/// risk of a 400.
///
/// The boundary is the same monotone staircase as every other rail
/// ([`history_prune::prune_boundary`]), so the request prefix stays byte-stable
/// for up to a full stride and OpenAI's automatic prompt cache keeps hitting.
///
/// Shared with the WebSocket bridge (#440) so Codex/WS turns prune identically.
pub(super) fn prune_responses_input(doc: &mut Value) -> bool {
    let Some(input) = doc.get_mut("input").and_then(|i| i.as_array_mut()) else {
        return false;
    };
    let boundary = super::history_prune::prune_boundary(input.len());
    if boundary == 0 {
        return false;
    }
    let tool_names = tool_kind::responses_tool_names(input);
    let mut modified = false;
    for item in input.iter_mut().take(boundary) {
        if item.get("type").and_then(|t| t.as_str()) != Some("function_call_output") {
            continue;
        }
        let kind = item
            .get("call_id")
            .and_then(|v| v.as_str())
            .and_then(|id| tool_names.get(id))
            .map_or(ToolResultKind::Other, |n| tool_kind::classify_tool_name(n));
        if let Some(output) = item.get_mut("output") {
            modified |= prune_output_field(output, kind);
        }
    }
    modified
}

/// Apply [`history_prune::prune_output_text`] to a `function_call_output.output`,
/// handling both the JSON-string and array-of-content-parts shapes — the
/// pruning analogue of [`compress_output_field`].
fn prune_output_field(output: &mut Value, kind: ToolResultKind) -> bool {
    super::tool_output::prune_value(output, kind)
}

/// Compresses the `function_call_output.output` entries of a Responses-API body
/// in place, returning whether anything changed. Shared by the HTTP handler and
/// the WebSocket bridge (#440) so both paths get identical, safe savings.
///
/// The only token sink we shrink is each `function_call_output.output` — the
/// Responses-API analogue of a Chat Completions `role:"tool"` message. We never
/// remove or reorder `input` items: the Responses API rejects a `function_call`
/// whose matching `function_call_output` is absent (and reasoning items must keep
/// their originating call), so all token reclamation happens *in place* on the
/// output text. Cache-aware pruning of the frozen OLD region lives in
/// [`prune_responses_input`]; this pass compresses whatever recent outputs remain.
pub(super) fn compress_responses_input(doc: &mut Value) -> bool {
    let cfg = crate::core::config::Config::load();
    let mut modified = false;
    if let Some(input) = doc.get_mut("input").and_then(|i| i.as_array_mut()) {
        let tool_names = tool_kind::responses_tool_names(input);
        for item in input.iter_mut() {
            if item.get("type").and_then(|t| t.as_str()) != Some("function_call_output") {
                continue;
            }
            let name = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .and_then(|id| tool_names.get(id))
                .map(String::as_str);
            // #481: per-tool exclusion (Serena default) — skip live compression
            // for excluded tools; history pruning above still applies.
            if name.is_some_and(|n| cfg.proxy.is_tool_live_compress_excluded(n)) {
                continue;
            }
            let kind = name.map_or(ToolResultKind::Other, tool_kind::classify_tool_name);
            if let Some(output) = item.get_mut("output") {
                modified |= compress_output_field(output, name, kind);
            }
        }
    }
    modified
}

/// Compress a `function_call_output.output`. OpenAI sends this as a JSON string,
/// but the API also accepts an array of content parts (`input_text` blocks) for
/// tools returning richer data, so both shapes are handled.
///
/// A protected file/source read (resolved from the matching `function_call`
/// name) is left intact so a mid-refactor model never loses the body it edits.
fn compress_output_field(
    output: &mut Value,
    tool_name: Option<&str>,
    kind: ToolResultKind,
) -> bool {
    super::tool_output::compress_value(output, tool_name, kind)
}

#[cfg(test)]
mod tests {
    use super::super::compress::compress_tool_result;
    use super::*;

    /// A long `git status` is a known-compressible fixture: `has_structural_output`
    /// is false for it, so it flows through the git-status pattern compressor.
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

    fn long_tool_json() -> String {
        let rows = (0..32)
            .map(|i| {
                serde_json::json!({
                    "path": format!("/Users/alex/work/app/src/module_{i}.rs"),
                    "regex": r"src/[a-z_]+\.rs:\d+",
                    "error": format!("error[E0{i:03}]: expected exact diagnostic text"),
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&serde_json::json!({ "results": rows })).unwrap()
    }

    #[test]
    fn shell_json_envelope_text_is_compressed() {
        let _lock = crate::core::data_dir::test_env_lock();
        let raw = long_git_status();
        let expected = compress_tool_result(&raw, Some("Bash"));
        let envelope = serde_json::to_string(&serde_json::json!({
            "content": [{"type": "text", "text": raw}],
            "isError": false,
        }))
        .unwrap();

        let body = serde_json::json!({
            "model": "gpt-5",
            "input": [
                {"type": "function_call", "call_id": "call_1", "name": "Bash", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "call_1", "output": envelope}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());

        assert!(comp < orig);
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let output = parsed["input"][1]["output"].as_str().unwrap();
        let envelope: Value = serde_json::from_str(output).unwrap();
        assert_eq!(envelope["content"][0]["text"].as_str().unwrap(), expected);
    }

    #[test]
    fn shell_json_envelope_non_text_field_is_compressed() {
        let _lock = crate::core::data_dir::test_env_lock();
        let raw = long_git_status();
        let expected = compress_tool_result(&raw, Some("Bash"));
        // Big shell output in a non-"text" field: the Shell/Search all-strings
        // rewrite must still reach it, while small scalar fields stay intact.
        let envelope = serde_json::to_string(&serde_json::json!({
            "stdout": raw,
            "exit_code": 0,
        }))
        .unwrap();

        let body = serde_json::json!({
            "model": "gpt-5",
            "input": [
                {"type": "function_call", "call_id": "call_1", "name": "Bash", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "call_1", "output": envelope}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());

        assert!(comp < orig, "non-text shell field should be compressed");
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let output = parsed["input"][1]["output"].as_str().unwrap();
        let envelope: Value = serde_json::from_str(output).unwrap();
        assert_eq!(envelope["stdout"].as_str().unwrap(), expected);
        assert_eq!(envelope["exit_code"].as_i64().unwrap(), 0);
    }

    #[test]
    fn old_shell_json_envelope_text_is_pruned() {
        let raw = long_git_status();
        let envelope = serde_json::to_string(&serde_json::json!({
            "content": [{"type": "text", "text": raw}],
            "isError": false,
        }))
        .unwrap();
        let mut output = Value::String(envelope);

        assert!(prune_output_field(&mut output, ToolResultKind::Shell));
        let envelope: Value = serde_json::from_str(output.as_str().unwrap()).unwrap();
        assert!(
            envelope["content"][0]["text"].as_str().unwrap().len() < raw.len(),
            "nested text payload should be pruned"
        );
    }

    #[test]
    fn string_output_mirrors_engine_and_shrinks() {
        // tee path depends on the data dir; serialize env access so a parallel
        // test never swaps LEAN_CTX_DATA_DIR between the two compressions (#498).
        let _lock = crate::core::data_dir::test_env_lock();
        let raw = long_git_status();
        let expected = compress_tool_result(&raw, None);
        assert!(
            expected.len() < raw.len(),
            "fixture must be compressible by the shared engine"
        );

        let body = serde_json::json!({
            "model": "gpt-5",
            "input": [
                {"type": "function_call_output", "call_id": "call_1", "output": raw}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());

        assert!(comp < orig, "compressed body must be smaller");
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["input"][0]["output"].as_str().unwrap(),
            expected,
            "output must be exactly what the shared compressor produces"
        );
    }

    #[test]
    fn array_output_text_is_compressed() {
        // tee path depends on the data dir; serialize env access so a parallel
        // test never swaps LEAN_CTX_DATA_DIR between the two compressions (#498).
        let _lock = crate::core::data_dir::test_env_lock();
        let raw = long_git_status();
        let expected = compress_tool_result(&raw, None);

        let body = serde_json::json!({
            "input": [
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": [{"type": "input_text", "text": raw}]
                }
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());

        assert!(comp < orig);
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["input"][0]["output"][0]["text"].as_str().unwrap(),
            expected
        );
    }

    #[test]
    fn non_tool_output_items_are_untouched() {
        let body = serde_json::json!({
            "input": [
                {"type": "message", "role": "user", "content": long_git_status()},
                {"type": "function_call", "call_id": "c", "name": "x", "arguments": "{}"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body.clone(), bytes.len());

        assert_eq!(comp, orig, "no function_call_output → passthrough");
        let reparsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reparsed, body);
    }

    #[test]
    fn plain_string_input_passthrough() {
        let body = serde_json::json!({"model": "gpt-5", "input": "hello world"});
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body.clone(), bytes.len());
        assert_eq!(comp, orig);
        let reparsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reparsed, body);
    }

    #[test]
    fn no_input_field_passthrough() {
        let body = serde_json::json!({"model": "gpt-5", "previous_response_id": "resp_abc"});
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body.clone(), bytes.len());
        assert_eq!(comp, orig);
        let reparsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reparsed, body);
    }

    #[test]
    fn chatgpt_responses_eval_fixture_keeps_exact_payloads_and_pairing() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let command_input = "$ cargo test --lib proxy::openai_responses\nerror[E0425]: cannot find value `x` in this scope\nsrc/proxy/openai_responses.rs:12:9";
        let body = serde_json::json!({"model": "gpt-5", "input": command_input});
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body.clone(), bytes.len());
        assert_eq!(comp, orig, "top-level exact command string must stay raw");
        assert_eq!(serde_json::from_slice::<Value>(&out).unwrap(), body);

        let old_json = long_tool_json();
        let recent_json = long_tool_json();
        let old_shell = long_git_status();
        let input_text_block = "```rust\nfn main() { panic!(\"exact\"); }\n```\nRegex: src/[a-z_]+\\.rs:\\d+\nPath: /Users/alex/work/app/src/main.rs\nError: error[E0425]: cannot find value `x` in this scope";
        let mut input = vec![
            serde_json::json!({"type": "reasoning", "id": "rs_1", "summary": []}),
            serde_json::json!({"type": "function_call", "call_id": "json_old", "name": "submit_tool_json", "arguments": "{\"strict\":true}"}),
            serde_json::json!({"type": "function_call_output", "call_id": "json_old", "output": old_json}),
            serde_json::json!({"type": "function_call", "call_id": "shell_old", "name": "Bash", "arguments": "{\"cmd\":\"git status\"}"}),
            serde_json::json!({"type": "function_call_output", "call_id": "shell_old", "output": old_shell}),
            serde_json::json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": input_text_block}]}),
        ];
        while input.len() < 22 {
            input.push(serde_json::json!({
                "type": "message",
                "role": "user",
                "content": format!("filler {}", input.len()),
            }));
        }
        input.push(serde_json::json!({"type": "function_call", "call_id": "json_recent", "name": "submit_tool_json", "arguments": "{\"strict\":true}"}));
        input.push(serde_json::json!({"type": "function_call_output", "call_id": "json_recent", "output": recent_json}));

        let body = serde_json::json!({"model": "gpt-5", "input": input});
        let item_count = body["input"].as_array().unwrap().len();
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());
        assert!(comp < orig, "old shell output should still provide savings");

        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let input = parsed["input"].as_array().unwrap();
        assert_eq!(input.len(), item_count, "no Responses item may be dropped");
        assert_eq!(input[0]["type"], "reasoning");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["output"].as_str().unwrap(), old_json);
        assert_ne!(input[4]["output"].as_str().unwrap(), old_shell);
        assert_eq!(
            input[5]["content"][0]["text"].as_str().unwrap(),
            input_text_block
        );
        assert_eq!(input[22]["type"], "function_call");
        assert_eq!(input[23]["type"], "function_call_output");
        assert_eq!(input[23]["output"].as_str().unwrap(), recent_json);
        assert_eq!(input[1]["call_id"], input[2]["call_id"]);
        assert_eq!(input[22]["call_id"], input[23]["call_id"]);
    }

    #[test]
    fn responses_instructions_and_assistant_prose_pass_through_verbatim() {
        // #710: role aggressiveness is off, so prose is never rewritten.
        let _iso = crate::core::data_dir::isolated_data_dir();

        let prose = big_prose();
        let body = serde_json::json!({
            "model": "gpt-5",
            "instructions": prose,
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "message", "role": "assistant", "content": prose},
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _orig, _comp) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(
            parsed["instructions"].as_str().unwrap(),
            prose,
            "Responses instructions must pass through verbatim"
        );
        assert_eq!(
            parsed["input"][1]["content"].as_str().unwrap(),
            prose,
            "assistant turns must pass through verbatim (#710)"
        );
    }

    #[test]
    fn responses_instructions_rewrite_is_deterministic() {
        let _iso = crate::core::data_dir::isolated_data_dir();

        let prose = big_prose();
        let mk = || {
            serde_json::json!({
                "model": "gpt-5",
                "instructions": prose,
                "input": [{"type": "message", "role": "user", "content": "hi"}],
            })
        };
        let (a, b) = (mk(), mk());
        let (la, lb) = (
            serde_json::to_vec(&a).unwrap().len(),
            serde_json::to_vec(&b).unwrap().len(),
        );
        assert_eq!(
            compress_request_body(a, la).0,
            compress_request_body(b, lb).0,
            "identical input must yield byte-identical output (#498)"
        );
    }

    #[test]
    fn short_output_unchanged() {
        let body = serde_json::json!({
            "input": [
                {"type": "function_call_output", "call_id": "c", "output": "ok"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body.clone(), bytes.len());
        assert_eq!(comp, orig);
        let reparsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reparsed, body);
    }

    /// `pairs` Responses turns: each is a `function_call` + its matching
    /// `function_call_output` carrying a long file read.
    fn responses_read_turns(pairs: usize) -> Vec<Value> {
        let code = (0..40)
            .map(|i| format!("    let v{i} = compute_{i}(ctx, opts);"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut input = Vec::new();
        for t in 0..pairs {
            input.push(serde_json::json!({
                "type": "function_call", "call_id": format!("c{t}"),
                "name": "read_file", "arguments": "{}"
            }));
            input.push(serde_json::json!({
                "type": "function_call_output", "call_id": format!("c{t}"),
                "output": format!("{code}\n// turn {t}")
            }));
        }
        input
    }

    #[test]
    fn cache_aware_prune_stubs_old_reads_keeps_recent_and_pairing() {
        // Default (isolated) config = cache-aware history mode.
        let _iso = crate::core::data_dir::isolated_data_dir();
        // 14 pairs = 28 items → staircase boundary 16.
        let body = serde_json::json!({"model": "gpt-5", "input": responses_read_turns(14)});
        let item_count = body["input"].as_array().unwrap().len();
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, orig, comp) = compress_request_body(body, bytes.len());
        assert!(comp < orig, "old reads must be pruned for savings");

        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let input = parsed["input"].as_array().unwrap();
        // Pairing + ordering preserved: not a single item dropped or moved.
        assert_eq!(input.len(), item_count, "no items may be removed (pairing)");
        for (i, item) in input.iter().enumerate() {
            let expect = if i.is_multiple_of(2) {
                "function_call"
            } else {
                "function_call_output"
            };
            assert_eq!(item["type"], expect, "item {i} type/order changed");
        }
        // An OLD file read (output index 1, before boundary 16) is stubbed.
        let old = input[1]["output"].as_str().unwrap();
        assert!(
            old.contains("Re-read the file"),
            "old read should be stubbed, got: {old}"
        );
        // A RECENT file read (output index 27, after the boundary) keeps its body.
        let recent = input[27]["output"].as_str().unwrap();
        assert!(
            recent.contains("v39"),
            "recent read must be protected, got: {recent}"
        );
    }

    #[test]
    fn responses_compression_is_deterministic() {
        // #498: the same request must compress to byte-identical output so the
        // provider's prompt cache (and our regression diffs) stay stable.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let mk = || serde_json::json!({"model": "gpt-5", "input": responses_read_turns(14)});
        let (a, b) = (mk(), mk());
        let (la, lb) = (
            serde_json::to_vec(&a).unwrap().len(),
            serde_json::to_vec(&b).unwrap().len(),
        );
        let (out_a, _, _) = compress_request_body(a, la);
        let (out_b, _, _) = compress_request_body(b, lb);
        assert_eq!(out_a, out_b, "identical input must yield identical bytes");
    }

    #[test]
    fn cache_aware_responses_prefix_is_byte_stable_across_turns() {
        // THE cache invariant for the Responses rail: as `input` grows turn by
        // turn, every item before an already-passed boundary must stay
        // byte-identical, or OpenAI's automatic prompt cache stops hitting.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let mut prev: Vec<String> = Vec::new();
        let mut prev_boundary = 0;
        for pairs in 1..=20 {
            let input = responses_read_turns(pairs);
            let len = input.len();
            let body = serde_json::json!({"model": "gpt-5", "input": input});
            let bytes = serde_json::to_vec(&body).unwrap();
            let (out, _, _) = compress_request_body(body, bytes.len());
            let parsed: Value = serde_json::from_slice(&out).unwrap();
            let items: Vec<String> = parsed["input"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::to_string)
                .collect();
            for i in 0..prev_boundary {
                assert_eq!(
                    prev[i], items[i],
                    "Responses item {i} changed at turn {pairs} — prompt cache prefix broken"
                );
            }
            prev = items;
            prev_boundary = crate::proxy::history_prune::prune_boundary(len);
        }
    }

    #[test]
    fn effort_control_is_off() {
        // #834: effort control is off, so no `reasoning` object is added.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({"model": "gpt-5.5", "input": []});
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        assert!(
            serde_json::from_slice::<Value>(&out)
                .unwrap()
                .get("reasoning")
                .is_none()
        );
    }
}
