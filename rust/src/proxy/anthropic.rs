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
use super::{cache_safety, prefix_cache_stats, prefix_replay, prose, sticky_tools};

std::thread_local! {
    /// Set by `forward.rs` when the current request has the `X-Headroom-Compressed` header.
    static HEADROOM_REQUEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Called from `forward.rs` to signal that this request was pre-compressed by Headroom.
pub(super) fn set_headroom_request(val: bool) {
    HEADROOM_REQUEST.set(val);
}
use crate::core::config::{HistoryMode, ProseRole};

pub async fn handler(
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
        "Anthropic",
        &[],
    )
    .await
}

pub(super) fn compress_request_body(
    parsed: Value,
    original_size: usize,
) -> (Vec<u8>, usize, usize) {
    let mut doc = parsed;
    let mut modified = false;

    // Opt-in per-role prose aggressiveness (#710). Both default to `None`, in
    // which case nothing below fires and the body is byte-for-byte unchanged.
    let cfg = crate::core::config::Config::load();
    let config_headroom = cfg.proxy.is_headroom_compat();
    if config_headroom {
        prefix_cache_stats::record_headroom_compat();
    }
    // Per-request Headroom detection: if this specific request carries the
    // X-Headroom-Compressed header, treat it as headroom-compat even without
    // the global config flag. The header is checked in forward.rs and the
    // result is threaded through via a thread-local set by the caller.
    let _headroom_compat = config_headroom || HEADROOM_REQUEST.get();
    let system_aggr = cfg.proxy.resolved_role_aggressiveness(ProseRole::System);
    let user_aggr = cfg.proxy.resolved_role_aggressiveness(ProseRole::User);
    let live_compress = cfg.proxy.live_compresses();
    let mode = cfg.proxy.resolved_history_mode();
    // #939: active prompt-cache breakpoint injection (opt-in, Anthropic-only).
    // Resolved up front so the meter-only short-circuit below does not skip the
    // one mutation this mode performs — its whole point is to add a cache anchor
    // to an otherwise byte-passthrough request.
    let inject_breakpoint = cfg.proxy.cache_breakpoint_enabled();
    // #940: cache-aligner volatile-field telemetry (default-on, measurement-only).
    // Also resolved up front so a meter-only proxy still reaches the scan slot —
    // it never mutates the body, it only records how much cache the system prompt
    // leaks, so it ships on for every proxy (#986 premium defaults).
    let align_volatile = cfg.proxy.cache_aligner_enabled();
    // #974: active cache-aligner relocate (opt-in, Anthropic-only). Resolved up
    // front like the telemetry above so a meter-only proxy still reaches the
    // relocate slot — this is the one mutation that moves volatile fields out of
    // the cacheable prefix.
    let relocate_volatile = cfg.proxy.cache_align_relocate_enabled();
    // #986: cache-economics (default-on). Resolved up front so the meter-only
    // short-circuit below still reaches the miss-attribution slot — that
    // telemetry only reads the cacheable prefix, it never mutates the body, and
    // the paired net-cost gate only makes the cold-prefix repack more
    // conservative, so both halves ship on for every proxy (#986 premium
    // defaults).
    let cache_economics = cfg.proxy.cache_policy_enabled();
    // #895 Track B: output-savings holdout arm, from the pristine body (before any
    // mutation below) so it matches the arm the response meter records. Control
    // conversations skip output-shaping (effort + verbosity steer) but are still
    // metered. Default holdout=0 → always Treatment (no behaviour change).
    let arm = super::holdout::assign(
        &super::holdout::anthropic_key(&doc),
        cfg.proxy.output_holdout_fraction(),
    );
    // #493: in-band CCR expansion (opt-in). Splice any <lc_expand:HASH> the model
    // echoed back into the verbatim original from the local tee store. A strict
    // no-op when no marker is present (byte-identical body → cache-safe). Runs
    // before the meter-only short-circuit so an explicit expand request is
    // honored even when the proxy is otherwise byte-passthrough.
    if cfg.proxy.ccr_inband_enabled() {
        modified |= super::ccr::splice_inband_in_place(&mut doc);
    }
    // #834: cache-safe cross-provider effort control. Default off → no-op. The
    // value is a constant, so it never perturbs the prompt-cache prefix; it only
    // dials an *existing* adaptive thinking request (never enables thinking the
    // client didn't ask for).
    if arm == super::holdout::Arm::Treatment {
        if let Some(effort) = cfg.proxy.resolved_effort() {
            modified |= super::effort::apply_anthropic(&mut doc, effort);
        }
        // #895: cache-safe wire verbosity steer (constant suffix after the last
        // cache_control breakpoint). Control arm skips it so the holdout measures
        // its effect.
        if cfg.proxy.verbosity_steer_enabled() {
            modified |= super::verbosity::apply_anthropic(&mut doc);
        }
    }
    // Meter-only (#481): live compression off, no history pruning, no prose
    // rewriting → forward + usage metering still run, but the body is left
    // unchanged so the provider prompt-cache prefix stays byte-stable. A pending
    // in-band splice (`modified`) opts out: the body did change this turn.
    if !live_compress
        && mode == HistoryMode::Off
        && system_aggr.is_none()
        && user_aggr.is_none()
        && !modified
        && !inject_breakpoint
        && !align_volatile
        && !relocate_volatile
        && !cache_economics
    {
        let out = serde_json::to_vec(&doc).unwrap_or_default();
        return (out, original_size, original_size);
    }
    let mut prose_segments: u64 = 0;

    // Length of the client's provider-cached message prefix. Needed both for
    // cache-safe pruning below and to gate top-level system prose: if any
    // message is client-cached, `system` (which precedes every message) is part
    // of that cached prefix and must not be rewritten.
    let cached = doc
        .get("messages")
        .and_then(|m| m.as_array())
        .map_or(0, |m| super::history_prune::cached_prefix_len(m));

    // #480: opt-in big-gap cold-prefix repack. When enabled AND the proxy can
    // confidently predict (from idle time vs the provider cache TTL) that the
    // client-cached prefix is already cold, override the normal "never touch the
    // cached prefix" rule for THIS request and prune/compress the prefix too,
    // re-seeding a leaner cache. Default-off; never fires without a measured idle
    // gap past TTL × margin, so warm caches stay byte-stable (#448).
    // #986: cache-economics miss attribution (opt-in, measurement-only). Classify
    // why this turn hits or misses the provider prompt-cache (TTL lapse vs prefix
    // change) and bump the `/status` gauges. Reads the cacheable prefix only — the
    // body is never touched — so it is strictly cache-safe.
    if cache_economics
        && let Some(m) = doc.get("messages").and_then(|m| m.as_array())
        && let Some(outcome) = super::cache_attribution::record_request(m, cached)
    {
        match outcome {
            super::cache_attribution::CacheOutcome::WarmReuse => {
                prefix_cache_stats::record_hit();
            }
            super::cache_attribution::CacheOutcome::ColdStart => {}
            _ => {
                prefix_cache_stats::record_miss();
            }
        }
    }
    // #480 repack decision, with the #986 net-cost gate folded in: when
    // cache-economics is on, also require the prefix to be large enough to cache
    // (`worth_repacking`). The gate is an extra AND-condition, so it can only make
    // repacking *more* conservative; default-off proxies keep the prior value.
    let repack = cfg.proxy.repacks_cold_prefix()
        && doc
            .get("messages")
            .and_then(|m| m.as_array())
            .is_some_and(|m| {
                super::cold_prefix::repack_decision(m, cached)
                    && (!cache_economics
                        || super::cache_policy::worth_repacking(doc.get("system"), m, cached))
            });
    // The prefix length the rewrites below must protect: the full cached prefix
    // normally, or 0 when we are intentionally repacking the cold prefix.
    let protect = if repack { 0 } else { cached };

    // System prose: only when nothing is client-cached and the `system` field
    // carries no `cache_control` of its own — otherwise it anchors the cache.
    // A cold-prefix repack (`protect == 0` with `repack`) deliberately rewrites
    // it to re-seed a leaner cache.
    let model_name = doc
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_owned();
    if let Some(a) = system_aggr
        && protect == 0
        && let Some(system) = doc.get_mut("system")
        && (repack || !prose::value_has_cache_control(system))
    {
        let should_compress = if repack || cached == 0 {
            true
        } else {
            let sys_tokens = prose::estimate_tokens(system) as u64;
            let estimated_after = (sys_tokens as f64 * (1.0 - a)).max(0.0) as u64;
            let reuse_rate = super::cache_attribution::estimated_reuse_rate();
            let model_cost = super::cache_policy::model_cost_for(&model_name);
            let gate = super::cache_policy::should_mutate_frozen(
                sys_tokens,
                estimated_after,
                reuse_rate,
                &model_cost,
            );
            matches!(gate, super::cache_policy::MutationDecision::Mutate { .. })
        };
        if should_compress {
            let n = prose::compress_system_value(system, a);
            if n > 0 {
                prose_segments += u64::from(n);
                modified = true;
            }
        }
    }

    if let Some(messages) = doc.get_mut("messages").and_then(|m| m.as_array_mut()) {
        // Resolve tool-call id → tool name so file/source reads can be protected
        // from lossy compression that would force the model to re-read mid-task.
        let tool_names = tool_kind::anthropic_tool_names(messages);

        // Prune at a frozen, cache-aware boundary by default: Anthropic's
        // prompt cache matches exact prefixes, so the boundary must not move
        // every turn (see `history_prune::prune_boundary`). `mode` resolved above.
        let boundary = super::history_prune::prune_boundary(mode, messages.len());
        // Never rewrite content the client has marked with `cache_control`:
        // pruning inside the already-cached prefix invalidates Anthropic's
        // prompt cache from the first changed message (#448). Pruning therefore
        // starts after the last breakpoint; with no breakpoint this is 0, i.e.
        // the previous behaviour.
        modified |=
            super::history_prune::prune_history_range(messages, protect, boundary, &tool_names);

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

                    // #481: skip live compression when globally off or when the
                    // originating tool is on the exclusion list (Serena default).
                    let excluded =
                        name.is_some_and(|n| cfg.proxy.is_tool_live_compress_excluded(n));
                    if live_compress
                        && !excluded
                        && let Some(inner_content) = block.get_mut("content")
                    {
                        modified |= compress_content_field(inner_content, name, kind);
                    }
                }
            }
        }

        // Frozen-region user prose: free-text `text` blocks of user turns in
        // `[cached, boundary)`. Cache-safe by construction — the cached prefix
        // and the live tail (`>= boundary`) are both left intact, and the
        // rewrite is content-deterministic so the prefix stays byte-stable.
        if let Some(a) = user_aggr {
            let end = boundary.min(messages.len());
            let start = protect.min(end);
            for msg in &mut messages[start..end] {
                if msg.get("role").and_then(|r| r.as_str()) == Some("user")
                    && let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut())
                {
                    prose_segments += u64::from(prose::compress_text_blocks(content, a));
                }
            }
        }
    }

    if prose_segments > 0 {
        modified = true;
    }
    // #940: cache-aligner telemetry. On an unanchored system prompt (the prefix a
    // provider would cache), count the volatile fields that would bust that cache
    // turn-to-turn. Pure measurement — runs before any breakpoint injection and
    // never mutates the body — so it is strictly cache-safe. Skipped once the
    // client has anchored the prefix itself.
    if align_volatile
        && cached == 0
        && let Some(system) = doc.get("system")
        && !prose::value_has_cache_control(system)
        && let Some(text) = super::cache_aligner::system_text(system)
    {
        let scan = super::cache_aligner::scan_volatile(&text);
        cache_safety::record_volatile_system(scan.fields as u64);
    }
    // #974: active cache-aligner relocate (Anthropic-only). After the telemetry
    // above measured the leak on the pristine prompt, move the volatile values out
    // of the cacheable prefix into an uncached tail block so the prefix finally
    // caches. Gated exactly like the breakpoint below — Treatment arm, and only
    // when the client anchored nothing of its own. The rewrite adds the
    // `cache_control` itself, so a following #939 injection sees an anchored prefix
    // and stays a no-op: the two compose to exactly one breakpoint on the stable
    // block, with the volatile tail left uncached.
    if relocate_volatile
        && arm == super::holdout::Arm::Treatment
        && cached == 0
        && doc
            .get("system")
            .is_some_and(|s| !prose::value_has_cache_control(s))
    {
        let relocated = super::cache_aligner::apply_anthropic_relocate(&mut doc);
        if relocated > 0 {
            modified = true;
            cache_safety::record_volatile_relocated(relocated as u64);
        }
    }
    // #939: active prompt-cache breakpoint injection (Anthropic-only). When the
    // client anchored no prefix of its own — no message `cache_control` (`cached
    // == 0`) and no breakpoint already on `system` — add one ephemeral breakpoint
    // to `system` so the large, stable system prompt bills later turns at the
    // cached rate (the win a raw API client leaves on the table). Runs after every
    // frozen-region rewrite so the marker anchors the final system bytes and the
    // prefix it creates stays byte-stable across turns (#498). Counted on its own
    // gauge — a pure win, never against the cache-safe ratio.
    if inject_breakpoint
        && cached == 0
        && doc
            .get("system")
            .is_some_and(|s| !prose::value_has_cache_control(s))
        && super::cache_breakpoint::inject_anthropic_system(&mut doc)
    {
        modified = true;
        cache_safety::record_breakpoint_injected();
    }
    // A deliberate cold-prefix repack (#480) is the one sanctioned exception to
    // the frozen-window rule; count it on its own gauge so it never dilutes the
    // cache-safe ratio (which exists to catch *accidental* #448 regressions).
    // Every other rewrite lands strictly inside the cache-safe frozen window.
    if repack {
        cache_safety::record_cold_repack();
    }
    cache_safety::record(prose_segments, true);

    // Sticky CCR tool injection: once a conversation has used CCR, keep
    // ctx_expand in tools[] to avoid prefix-cache-busting tool-list changes.
    let system_val = doc.get("system");
    let messages_for_id = doc.get("messages").and_then(Value::as_array);
    if let Some(msgs) = messages_for_id {
        let conv_id = prefix_replay::conversation_id(system_val, msgs);
        if sticky_tools::ensure_tool_present(conv_id, &mut doc) {
            modified = true;
            prefix_cache_stats::record_sticky_injection();
        }
    }

    prefix_cache_stats::record_frozen_count(cached as u64);

    // Prefix replay: if this is an append-only turn, overlay the cached
    // forwarded prefix bytes with the fresh delta for byte-identical prefix.
    let system_val_replay = doc.get("system");
    let msgs_replay = doc.get("messages").and_then(Value::as_array);
    let out = if let Some(msgs) = msgs_replay {
        let conv_id = prefix_replay::conversation_id(system_val_replay, msgs);
        if let Some(delta) = prefix_replay::detect_append_only(conv_id, msgs) {
            let delta_msgs = &msgs[delta.delta_start..];
            if let Some(replayed) = prefix_replay::overlay_prefix(&delta.prefix_bytes, delta_msgs) {
                prefix_cache_stats::record_replay_hit();
                let original_bytes = serde_json::to_vec(&doc).unwrap_or_default();
                prefix_cache_stats::record_delta(
                    original_bytes.len() as u64,
                    replayed.len() as u64,
                );
                replayed
            } else {
                prefix_cache_stats::record_replay_miss();
                serde_json::to_vec(&doc).unwrap_or_default()
            }
        } else {
            prefix_cache_stats::record_replay_miss();
            serde_json::to_vec(&doc).unwrap_or_default()
        }
    } else {
        serde_json::to_vec(&doc).unwrap_or_default()
    };
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
        // test never swaps LEAN_CTX_DATA_DIR between the two compressions.
        let _lock = crate::core::data_dir::test_env_lock();
        // #498: the proxy rewrite must be a pure function of the body so the
        // provider prompt-cache prefix stays byte-identical across turns.
        let bytes = serde_json::to_vec(&forge_log_body("Bash")).unwrap();
        let a = compress_request_body(serde_json::from_slice(&bytes).unwrap(), bytes.len()).0;
        let b = compress_request_body(serde_json::from_slice(&bytes).unwrap(), bytes.len()).0;
        assert_eq!(a, b, "identical input must yield byte-identical output");
    }

    /// A large, highly-compressible foreign log so the live path tees + stubs it.
    fn big_log() -> String {
        (0..200)
            .map(|i| format!("[info] processed item {i:04} ok, latency {i}ms, queue normal"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn inband_ccr_emit_echo_splice_round_trip() {
        // Full #493 cycle through the real Anthropic request path: a lossy stub
        // emits an <lc_expand:HASH> marker, the model echoes it, and the proxy
        // splices the verbatim original back inline on the next request.
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.ccr_inband = Some(true);
        })
        .unwrap();

        // EMIT: live-compress a foreign tool_result → recovery stub with a marker.
        let log = big_log();
        let emit = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "bash", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": log}]}
            ]
        });
        let bytes = serde_json::to_vec(&emit).unwrap();
        let (out, _o, _c) = compress_request_body(emit, bytes.len());
        let emitted: Value = serde_json::from_slice(&out).unwrap();
        let stub = emitted["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            stub.contains("<lc_expand:"),
            "in-band stub must advertise an echo-able marker: {stub}"
        );
        assert!(
            !stub.contains("/tee/proxy_"),
            "in-band stub must not leak the unreachable local tee path: {stub}"
        );

        // The marker the model would copy into its next turn.
        let start = stub.find("<lc_expand:").unwrap();
        let end = stub[start..].find('>').unwrap() + start + 1;
        let marker = &stub[start..end];

        // ECHO + SPLICE: the model echoes the marker; the proxy splices the
        // verbatim original (recovered from the local tee store) back inline.
        let echo = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "look again"}]},
                {"role": "assistant", "content": format!("revisiting that output: {marker}")}
            ]
        });
        let bytes = serde_json::to_vec(&echo).unwrap();
        let (out, _o, _c) = compress_request_body(echo, bytes.len());
        let spliced: Value = serde_json::from_slice(&out).unwrap();
        let assistant = spliced["messages"][1]["content"].as_str().unwrap();
        assert!(
            assistant.contains("processed item 0007 ok")
                && assistant.contains("processed item 0199 ok"),
            "the verbatim original must be spliced back in full: {assistant}"
        );
        assert!(
            !assistant.contains("<lc_expand:"),
            "the marker must be consumed by the splice"
        );
    }

    #[test]
    fn inband_marker_less_turn_is_byte_identical_on_or_off() {
        // Cache-safety (#493): enabling in-band must be a strict no-op on a turn
        // with no marker — same bytes on the wire, so the provider cache prefix is
        // never perturbed unless the model actually asked to expand. Uses a body
        // with nothing to prune/compress, isolating the splice from stub emission.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello there"}]},
                {"role": "assistant", "content": "hi — how can I help?"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();

        crate::core::config::Config::update_global(|c| c.proxy.ccr_inband = Some(false)).unwrap();
        let off = compress_request_body(body.clone(), bytes.len()).0;
        crate::core::config::Config::update_global(|c| c.proxy.ccr_inband = Some(true)).unwrap();
        let on = compress_request_body(body, bytes.len()).0;

        assert_eq!(
            off, on,
            "a marker-less request must be byte-identical whether in-band is on or off"
        );
    }

    /// Long, duplicate-rich natural-language prose that compresses cleanly.
    fn big_prose() -> String {
        let p = "You are a careful, senior software engineer. You always explain your \
                 reasoning before making changes, you prefer small reviewable diffs, and \
                 you never introduce mock data or placeholders into production code. ";
        [p; 6].join("\n")
    }

    #[test]
    fn system_prose_compressed_and_assistant_untouched() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.system = Some(0.6);
            c.proxy.role_aggressiveness.user = Some(0.6);
        })
        .unwrap();

        let prose = big_prose();
        let assistant_text = big_prose();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": prose,
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": prose}]},
                {"role": "assistant", "content": assistant_text},
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _orig, _comp) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            parsed["system"].as_str().unwrap().len() < prose.len(),
            "system prose must be compressed when enabled"
        );
        assert_eq!(
            parsed["messages"][1]["content"].as_str().unwrap(),
            assistant_text,
            "assistant turns must pass through verbatim (#710)"
        );
    }

    #[test]
    fn user_prose_compressed_only_in_frozen_region() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.user = Some(0.7);
        })
        .unwrap();

        let prose = big_prose();
        // 30 messages → cache-aware boundary = ((30-8)/16)*16 = 16.
        let mut messages = Vec::new();
        for i in 0..30 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            messages.push(serde_json::json!({
                "role": role,
                "content": [{"type": "text", "text": prose}]
            }));
        }
        let body = serde_json::json!({ "messages": messages });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        let frozen_user = parsed["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            frozen_user.len() < prose.len(),
            "user prose in the frozen region must be compressed"
        );
        assert_eq!(
            parsed["messages"][1]["content"][0]["text"]
                .as_str()
                .unwrap(),
            prose,
            "assistant prose is never compressed"
        );
        let live_tail_user = parsed["messages"][28]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert_eq!(
            live_tail_user, prose,
            "user prose in the live tail (>= boundary) must be preserved for quality"
        );
    }

    #[test]
    fn client_cached_prefix_disables_system_prose() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.system = Some(0.9);
        })
        .unwrap();

        let prose = big_prose();
        let body = serde_json::json!({
            "system": prose,
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "hi", "cache_control": {"type": "ephemeral"}}
                ]},
                {"role": "assistant", "content": "ok"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["system"].as_str().unwrap(),
            prose,
            "system must stay verbatim when the client caches a message prefix (#448)"
        );
    }

    #[test]
    fn prose_compression_is_deterministic() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.system = Some(0.6);
        })
        .unwrap();
        let prose = big_prose();
        let mk = || serde_json::json!({"system": prose, "messages": [{"role": "user", "content": "hi"}]});
        let (a, b) = (mk(), mk());
        let la = serde_json::to_vec(&a).unwrap().len();
        let lb = serde_json::to_vec(&b).unwrap().len();
        assert_eq!(
            compress_request_body(a, la).0,
            compress_request_body(b, lb).0,
            "prose compression must be byte-identical for identical input (#498)"
        );
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

    /// A client-cached message anchors the prefix; `system` precedes it, so the
    /// cached prefix is `cached > 0` and system prose is normally protected.
    /// System-prose verbatim-vs-rewritten is therefore a clean binary signal for
    /// whether the #480 cold-prefix repack fired.
    ///
    /// `first_text` must be UNIQUE per test: it is `messages[0]`, which the
    /// cold-prefix tracker hashes into the conversation key. A shared global
    /// last-touch store has no test-clear hook (that would race with the unit
    /// tests), so distinct keys are how parallel tests stay isolated.
    fn cached_prefix_body(first_text: &str, prose: &str) -> (Vec<Value>, Value) {
        let messages = vec![
            serde_json::json!({"role": "user", "content": [
                {"type": "text", "text": first_text, "cache_control": {"type": "ephemeral"}}
            ]}),
            serde_json::json!({"role": "assistant", "content": "ok"}),
        ];
        let body = serde_json::json!({ "system": prose, "messages": messages.clone() });
        (messages, body)
    }

    #[test]
    fn cold_prefix_repack_rewrites_protected_system_prose_when_enabled() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.system = Some(0.9);
            c.proxy.cold_prefix_repack = Some(true);
        })
        .unwrap();

        // The prefix must clear the cacheable floor: with premium defaults the
        // net-cost gate (#986, on by default) skips repacking a sub-1024-token
        // prefix the provider could never cache. A real cold prefix worth
        // re-seeding is large, so size the system prose accordingly.
        let prose = big_prose().repeat(6);
        let (messages, body) = cached_prefix_body("cold-repack-enabled-session", &prose);
        // Predict cold: last touched 3h ago, well past the 5m default TTL × margin.
        super::super::cold_prefix::test_seed_last_touch(&messages, 3 * 60 * 60);

        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            parsed["system"].as_str().unwrap().len() < prose.len(),
            "a predicted-cold prefix must let the proxy repack the otherwise-protected system prose"
        );
    }

    #[test]
    fn cold_prefix_repack_skipped_for_subcacheable_prefix_by_default() {
        // #986 premium default: cache_policy is on, so the net-cost gate skips a
        // cold repack of a prefix below the provider's cacheable minimum —
        // re-seeding it could never produce a cache the provider keeps. Repack is
        // enabled and the prefix is cold, but it is too small (≈345 tokens) to
        // cache, so the system prose must stay protected (unchanged).
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.system = Some(0.9);
            c.proxy.cold_prefix_repack = Some(true);
        })
        .unwrap();

        let prose = big_prose();
        let (messages, body) = cached_prefix_body("cold-repack-subcacheable-session", &prose);
        super::super::cold_prefix::test_seed_last_touch(&messages, 3 * 60 * 60);

        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["system"].as_str().unwrap(),
            prose,
            "the net-cost gate must skip repacking a sub-cacheable prefix (premium default)"
        );
    }

    #[test]
    fn cold_prefix_repack_off_by_default_keeps_prefix_protected() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.system = Some(0.9);
            c.proxy.cold_prefix_repack = Some(false);
        })
        .unwrap();

        let prose = big_prose();
        let (messages, body) = cached_prefix_body("cold-repack-disabled-session", &prose);
        // Even with a huge idle gap, default-off must never touch the prefix.
        super::super::cold_prefix::test_seed_last_touch(&messages, 24 * 60 * 60);

        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["system"].as_str().unwrap(),
            prose,
            "with repack off the cached prefix stays byte-stable regardless of idle time (#448)"
        );
    }

    #[test]
    fn cold_prefix_repack_protects_warm_prefix_even_when_enabled() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.role_aggressiveness.system = Some(0.9);
            c.proxy.cold_prefix_repack = Some(true);
        })
        .unwrap();

        let prose = big_prose();
        let (messages, body) = cached_prefix_body("cold-repack-warm-session", &prose);
        // Warm: touched 1 minute ago → the prediction must keep protecting.
        super::super::cold_prefix::test_seed_last_touch(&messages, 60);

        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["system"].as_str().unwrap(),
            prose,
            "a warm prefix must stay protected even with repack enabled — only LARGE gaps trigger"
        );
    }

    #[test]
    fn cache_policy_attribution_is_measurement_only() {
        // #986: enabling cache-economics records miss-attribution telemetry but
        // must never change the bytes on the wire — the same request compressed
        // with the policy off vs on is byte-identical (strictly cache-safe).
        let _iso = crate::core::data_dir::isolated_data_dir();

        let prose = big_prose();
        let (_messages, body) = cached_prefix_body("cache-policy-measurement-session", &prose);
        let bytes = serde_json::to_vec(&body).unwrap();

        crate::core::config::Config::update_global(|c| {
            c.proxy.cache_policy = Some(false);
        })
        .unwrap();
        let (off, _o, _c) = compress_request_body(body.clone(), bytes.len());

        crate::core::config::Config::update_global(|c| {
            c.proxy.cache_policy = Some(true);
        })
        .unwrap();
        let (on, _o, _c) = compress_request_body(body, bytes.len());

        assert_eq!(
            off, on,
            "miss attribution is measurement-only: the wire bytes must not change"
        );
    }

    #[test]
    fn effort_control_dials_adaptive_thinking_only() {
        // #834 end-to-end: fill output_config.effort when the client already
        // asked for adaptive thinking, but never enable thinking otherwise.
        let _iso = crate::core::data_dir::isolated_data_dir();
        crate::core::config::Config::update_global(|c| {
            c.proxy.effort = Some("medium".into());
        })
        .unwrap();

        let adaptive = serde_json::json!({
            "model": "claude-opus-4-8",
            "thinking": {"type": "adaptive"},
            "messages": [{"role": "user", "content": "hi"}]
        });
        let bytes = serde_json::to_vec(&adaptive).unwrap();
        let (out, _o, _c) = compress_request_body(adaptive, bytes.len());
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap()["output_config"]["effort"],
            "medium"
        );

        // No thinking field → the proxy must not add output_config (no surprise
        // reasoning cost, no 400 risk).
        let plain = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [{"role": "user", "content": "hi"}]
        });
        let bytes = serde_json::to_vec(&plain).unwrap();
        let (out, _o, _c) = compress_request_body(plain, bytes.len());
        assert!(
            serde_json::from_slice::<Value>(&out)
                .unwrap()
                .get("output_config")
                .is_none()
        );
    }

    #[test]
    fn verbosity_steer_applies_to_treatment_skips_control() {
        // #895: the holdout control arm must be byte-unchanged (so its output is
        // the measurement baseline); the treatment arm gets the constant steer.
        let _iso = crate::core::data_dir::isolated_data_dir();

        let req = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [{"role": "user", "content": "Summarize the design."}]
        });
        let bytes = serde_json::to_vec(&req).unwrap();

        // Steer on, holdout = 0 → everyone Treatment → steered.
        crate::core::config::Config::update_global(|c| {
            c.proxy.verbosity_steer = Some(true);
            c.proxy.output_holdout = Some(0.0);
        })
        .unwrap();
        let (out, _o, _c) = compress_request_body(req.clone(), bytes.len());
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            v["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains(crate::proxy::verbosity::STEER),
            "treatment arm must receive the verbosity steer"
        );

        // Steer on, holdout = 1.0 → everyone Control → byte-unchanged, no steer.
        crate::core::config::Config::update_global(|c| {
            c.proxy.output_holdout = Some(1.0);
        })
        .unwrap();
        let (out2, _o, _c) = compress_request_body(req, bytes.len());
        let v2: Value = serde_json::from_slice(&out2).unwrap();
        assert!(
            !v2["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains(crate::proxy::verbosity::STEER),
            "control arm must NOT be steered (measurement baseline)"
        );
    }

    /// A system prompt comfortably over Anthropic's minimum cacheable size, so
    /// the #939 breakpoint gate fires.
    fn cacheable_system() -> String {
        "You are a careful, senior software engineer who writes maintainable code. ".repeat(400)
    }

    #[test]
    fn cache_breakpoint_injected_on_unanchored_system_when_opt_in() {
        // #939: opt-in on AND the client set no cache_control of its own → exactly
        // one ephemeral breakpoint lands on `system`, wrapping the verbatim text.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();

        crate::core::config::Config::update_global(|c| c.proxy.cache_breakpoint = Some(true))
            .unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let v: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(
            v["system"][0]["cache_control"]["type"], "ephemeral",
            "an unanchored system prompt must receive one ephemeral breakpoint"
        );
        assert!(
            v["system"][0]["text"]
                .as_str()
                .unwrap()
                .contains("senior software engineer"),
            "the system text must be preserved verbatim under the marker"
        );
    }

    #[test]
    fn cache_breakpoint_off_by_default_is_byte_unchanged() {
        // Default off → the request must be byte-identical (no system reshape),
        // preserving the meter-only / cache-stable contract.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        assert_eq!(
            out, bytes,
            "default-off must leave the request byte-identical"
        );
    }

    #[test]
    fn cache_breakpoint_respects_client_anchor() {
        // #939 safety: a client cache_control on a message means `system` is part
        // of the already-cached prefix — never add a second, prefix-shifting anchor.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system(),
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello",
                    "cache_control": {"type": "ephemeral"}
                }]
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        crate::core::config::Config::update_global(|c| c.proxy.cache_breakpoint = Some(true))
            .unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            v["system"].is_string(),
            "with a client anchor present, system must be left untouched (no second breakpoint)"
        );
    }

    #[test]
    fn cache_aligner_measures_without_mutating_body() {
        // #940: the volatile-field scan is telemetry-only — enabling it must leave
        // the request byte-identical (measurement, not a rewrite), even on a
        // volatile-field-rich system prompt.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": "Today is 2026-06-22. Session 550e8400-e29b-41d4-a716-446655440000.",
            "messages": [{"role": "user", "content": "Hello."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        crate::core::config::Config::update_global(|c| c.proxy.cache_aligner = Some(true)).unwrap();
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

    fn clear_relocate_env() {}

    #[test]
    fn cache_align_relocate_moves_volatiles_to_tail_when_opt_in() {
        // #974: opt-in on, client anchored nothing → the date leaves the cacheable
        // prefix for an uncached tail block, and the stable block carries the one
        // ephemeral breakpoint.
        let _iso = crate::core::data_dir::isolated_data_dir();
        clear_relocate_env();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system_with_date(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        crate::core::config::Config::update_global(|c| {
            c.proxy.cache_align_relocate = Some(true);
            c.proxy.output_holdout = Some(0.0);
        })
        .unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let v: Value = serde_json::from_slice(&out).unwrap();

        assert!(v["system"].is_array(), "system reshaped into a block array");
        assert_eq!(v["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(
            !v["system"][0]["text"]
                .as_str()
                .unwrap()
                .contains("2026-06-27"),
            "the volatile date must leave the cacheable prefix"
        );
        assert!(
            v["system"][1].get("cache_control").is_none(),
            "the relocated tail block stays uncached"
        );
        assert!(
            v["system"][1]["text"]
                .as_str()
                .unwrap()
                .contains("2026-06-27"),
            "the date must be re-stated in the tail"
        );
    }

    #[test]
    fn cache_align_relocate_off_by_default_is_byte_unchanged() {
        // Default off → byte-identical request, preserving the cache-stable
        // contract even with a volatile-rich system prompt.
        let _iso = crate::core::data_dir::isolated_data_dir();
        clear_relocate_env();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system_with_date(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        assert_eq!(
            out, bytes,
            "default-off must leave the request byte-identical"
        );
    }

    #[test]
    fn cache_align_relocate_skips_control_arm() {
        // #895 holdout: a control-arm conversation must be byte-unchanged so its
        // cache behaviour is the measurement baseline.
        let _iso = crate::core::data_dir::isolated_data_dir();
        clear_relocate_env();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system_with_date(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        crate::core::config::Config::update_global(|c| {
            c.proxy.cache_align_relocate = Some(true);
            c.proxy.output_holdout = Some(1.0);
        })
        .unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            v["system"].is_string(),
            "control arm must not be relocated (measurement baseline)"
        );
    }

    #[test]
    fn cache_align_relocate_respects_client_anchor() {
        // Safety: a client cache_control means `system` is part of the cached
        // prefix — never relocate it (that would shift the cached prefix, #448).
        let _iso = crate::core::data_dir::isolated_data_dir();
        clear_relocate_env();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system_with_date(),
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello",
                    "cache_control": {"type": "ephemeral"}
                }]
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        crate::core::config::Config::update_global(|c| {
            c.proxy.cache_align_relocate = Some(true);
            c.proxy.output_holdout = Some(0.0);
        })
        .unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            v["system"].is_string(),
            "with a client anchor present, system must be left untouched"
        );
    }

    #[test]
    fn cache_align_relocate_composes_with_breakpoint_to_one_anchor() {
        // Both opt-ins on: relocate adds the breakpoint to the stable block, so the
        // #939 injection sees an anchored prefix and stays a no-op — exactly one
        // breakpoint, on the stable block, with the volatile tail uncached.
        let _iso = crate::core::data_dir::isolated_data_dir();
        clear_relocate_env();
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "system": cacheable_system_with_date(),
            "messages": [{"role": "user", "content": "Refactor the parser."}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        crate::core::config::Config::update_global(|c| {
            c.proxy.cache_align_relocate = Some(true);
            c.proxy.cache_breakpoint = Some(true);
            c.proxy.output_holdout = Some(0.0);
        })
        .unwrap();
        let (out, _o, _c) = compress_request_body(body, bytes.len());
        let v: Value = serde_json::from_slice(&out).unwrap();
        let blocks = v["system"].as_array().expect("system is a block array");
        assert_eq!(blocks.len(), 2, "stable block + volatile tail");
        assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
        assert!(
            blocks[1].get("cache_control").is_none(),
            "no second breakpoint — the tail stays uncached"
        );
    }
}
