//! Active request router (enterprise#13) — alias + intent-tier model rewrite
//! in the forward path, **fail-open by construction**.
//!
//! Runs between body parse and body compression: it may replace the `model`
//! field and re-target the request to another upstream of the **same wire
//! shape** — or, with the `shape-xlat` feature (enterprise#16), route an
//! Anthropic `/v1/messages` request onto an OpenAI-shape upstream with the
//! translation flag set. The decision is recorded as `routed_from` on the
//! usage record, so savings attribution can prove what the router did
//! (enterprise#15/#19).
//!
//! Two rule sources (`[proxy.routing]`, [`RoutingRules`]):
//!
//! 1. **Aliases** — exact requested-model match. `"acme/fast" = "foundry:gpt-4o-mini"`
//!    gives clients a stable org-level name; `"claude-opus-4-5" = "claude-sonnet-4-5"`
//!    transparently downgrades a concrete model.
//! 2. **Tiers** — intent classification of the request's last user message
//!    (`intent_engine::classify` → `route_intent` → `fast|standard|premium`)
//!    picks the target from the `tiers` table. Unset/empty tier = keep the
//!    requested model.
//!
//! Every failure mode — no rules, no model field, unknown target provider,
//! shape mismatch, unextractable query — routes nothing: the request forwards
//! unchanged. A routing bug can cost savings, never availability.

use crate::core::config::{RoutingRules, Upstreams, WireShape, parse_route_target};

/// What the router decided for one request. Applied by the forward path:
/// `model` already swapped in the body by [`route_request`]; the caller
/// re-targets the upstream and injects the registry credential if set.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RouteDecision {
    /// Model now in the body.
    pub model: String,
    /// Originally requested model (usage record `routed_from`).
    pub routed_from: String,
    /// Registry/builtin provider id serving the request after routing
    /// (usage attribution); `None` = upstream unchanged.
    pub provider_id: Option<String>,
    /// Override for the upstream base URL; `None` = keep the handler's.
    pub upstream_base: Option<String>,
    /// Target's local-inference flag (shadow-rate billing): `Some` for
    /// registry targets, `None` for built-ins (URL heuristic applies).
    pub local: Option<bool>,
    /// Cross-shape route (enterprise#16, feature `shape-xlat`): the Anthropic
    /// request body must be translated to OpenAI Chat Completions before it
    /// leaves, and the response translated back. Always `false` within-shape.
    pub xlat: bool,
}

/// Maximum user-message prefix fed to the intent classifier. Classification is
/// keyword/structure based; a bounded prefix keeps it O(1) per request.
#[allow(dead_code)]
const CLASSIFY_QUERY_CAP: usize = 2000;

/// Applies the routing rules to a parsed request body. On a routing decision
/// the body's `model` field is rewritten in place and the full decision is
/// returned; on any miss/failure the body is untouched and `None` is returned
/// (fail-open passthrough).
///
/// `xlat_ok` — the caller vouches that this request may be shape-translated
/// (exact messages-create path, `shape-xlat` compiled in). Subpaths like
/// `count_tokens`/`batches` have no OpenAI equivalent and must stay
/// within-shape.
pub(crate) fn route_request(
    parsed: &mut serde_json::Value,
    provider_label: &str,
    upstreams: &Upstreams,
    rules: &RoutingRules,
    xlat_ok: bool,
) -> Option<RouteDecision> {
    if !rules.is_active() {
        return None;
    }
    // Body-addressed model dialects route. Gemini keys the model in the URL
    // path, so it passes through.
    let request_shape = match provider_label {
        "Anthropic" => WireShape::Anthropic,
        "OpenAI" => WireShape::OpenAi,
        _ => return None,
    };
    let requested = parsed.get("model")?.as_str()?.trim().to_string();
    if requested.is_empty() {
        return None;
    }

    let target = rules.aliases.get(&requested).cloned().or_else(|| {
        let content_ref = extract_user_query(parsed, request_shape)?;
        // The router keys on a message body, so the extracted query is handed
        // back in one.
        let body = serde_json::json!({
            "model": requested.clone(),
            "messages": [{"role": "user", "content": content_ref}]
        });
        let decision = super::model_router::route(&body, rules)?;
        if decision.routed_model == requested {
            return None;
        }
        let provider = decision
            .routed_provider
            .unwrap_or_else(|| infer_provider(&decision.routed_model));
        Some(format!("{provider}:{}", decision.routed_model))
    })?;
    let (provider, new_model) = parse_route_target(&target)?;
    let new_model = new_model.to_string();

    let resolved = match provider {
        None => ResolvedTarget::default(),
        Some(p) => resolve_provider(p, request_shape, upstreams, xlat_ok)?,
    };

    if new_model == requested && resolved.upstream_base.is_none() {
        return None; // no-op rule
    }

    parsed["model"] = serde_json::Value::String(new_model.clone());
    let decision = RouteDecision {
        model: new_model,
        routed_from: requested,
        provider_id: resolved.provider_id,
        upstream_base: resolved.upstream_base,
        local: resolved.local,
        xlat: resolved.xlat,
    };
    Some(decision)
}

/// A resolved route target. `Default` = model-only rewrite (upstream unchanged).
#[derive(Default)]
struct ResolvedTarget {
    provider_id: Option<String>,
    upstream_base: Option<String>,
    local: Option<bool>,
    xlat: bool,
}

/// Resolves a route-target provider name, enforcing the shape rules: same
/// shape always routes; Anthropic→OpenAI routes with the translation flag when
/// the `shape-xlat` feature is compiled in and the caller allowed it. Unknown
/// ids and untranslatable shape pairs are logged and route nothing.
fn resolve_provider(
    name: &str,
    request_shape: WireShape,
    upstreams: &Upstreams,
    xlat_ok: bool,
) -> Option<ResolvedTarget> {
    let (target_shape, base_url, local) = match name {
        "anthropic" => (WireShape::Anthropic, upstreams.anthropic.clone(), None),
        "openai" => (WireShape::OpenAi, upstreams.openai.clone(), None),
        "gemini" => (WireShape::Gemini, upstreams.gemini.clone(), None),
        id => {
            let Some(p) = upstreams.provider_by_id(id) else {
                tracing::warn!("[proxy.routing] unknown target provider '{id}' — passthrough");
                return None;
            };
            (p.shape, p.base_url.clone(), Some(p.local))
        }
    };
    let xlat = if target_shape == request_shape {
        false
    } else if can_translate(request_shape, target_shape, xlat_ok, local) {
        true
    } else {
        tracing::warn!(
            "[proxy.routing] target '{name}' speaks {} but the request is {} — \
             not translatable here, passthrough",
            target_shape.as_str(),
            request_shape.as_str()
        );
        return None;
    };
    Some(ResolvedTarget {
        provider_id: Some(name.to_string()),
        upstream_base: Some(base_url),
        local,
        xlat,
    })
}

/// Anthropic→OpenAI is the supported translation pair (enterprise#16). The
/// upstream must be a local endpoint (no auth) — the caller's Anthropic
/// credentials mean nothing to an OpenAI-shape provider.
#[cfg(feature = "shape-xlat")]
fn can_translate(
    request_shape: WireShape,
    target_shape: WireShape,
    xlat_ok: bool,
    local: Option<bool>,
) -> bool {
    xlat_ok
        && request_shape == WireShape::Anthropic
        && target_shape == WireShape::OpenAi
        && local == Some(true)
}

#[cfg(not(feature = "shape-xlat"))]
fn can_translate(
    _request_shape: WireShape,
    _target_shape: WireShape,
    _xlat_ok: bool,
    _local: Option<bool>,
) -> bool {
    false
}

/// Extracts the newest user-authored text from a request body — the router's
/// classification input. Handles the two body-addressed dialects:
///
/// - Anthropic Messages / OpenAI Chat: `messages[]`, last `role == "user"`,
///   content as string or text-part array.
/// - OpenAI Responses: `input` as string, or `input[]` items with
///   `role == "user"` and `content[]` parts (`input_text`/`text`).
#[allow(dead_code)]

/// Provider a model name implies when the routing rules name none. Moved here
/// with the call site: it never returns an empty string, so the routed target
/// is always `provider:model`.
fn infer_provider(model: &str) -> String {
    if model.contains("gpt") || model.contains("o1") || model.contains("o3") {
        "openai".to_string()
    } else if model.contains("claude") {
        "anthropic".to_string()
    } else if model.contains("gemini") {
        "google".to_string()
    } else {
        "unknown".to_string()
    }
}
fn extract_user_query(parsed: &serde_json::Value, shape: WireShape) -> Option<String> {
    debug_assert!(matches!(shape, WireShape::Anthropic | WireShape::OpenAi));
    let items = parsed.get("messages").or_else(|| parsed.get("input"))?;

    // OpenAI Responses shorthand: `"input": "plain text"`.
    if let Some(text) = items.as_str() {
        return non_empty_prefix(text);
    }
    let items = items.as_array()?;
    let last_user = items.iter().rev().find(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some("user")
            || (m.get("type").and_then(|t| t.as_str()) == Some("message")
                && m.get("role").and_then(|r| r.as_str()) == Some("user"))
    })?;
    let content = last_user.get("content")?;
    if let Some(text) = content.as_str() {
        return non_empty_prefix(text);
    }
    let parts = content.as_array()?;
    let mut buf = String::new();
    for part in parts {
        let is_text = matches!(
            part.get("type").and_then(|t| t.as_str()),
            Some("text" | "input_text")
        );
        if is_text && let Some(t) = part.get("text").and_then(|t| t.as_str()) {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(t);
            if buf.len() >= CLASSIFY_QUERY_CAP {
                break;
            }
        }
    }
    non_empty_prefix(&buf)
}

#[allow(dead_code)]
fn non_empty_prefix(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut end = trimmed.len().min(CLASSIFY_QUERY_CAP);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(trimmed[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::ResolvedProvider;
    use serde_json::json;

    fn upstreams_with_foundry() -> Upstreams {
        Upstreams {
            anthropic: "https://api.anthropic.com".into(),
            openai: "https://api.openai.com".into(),
            gemini: "https://generativelanguage.googleapis.com".into(),
            providers: vec![
                ResolvedProvider {
                    id: "foundry".into(),
                    shape: WireShape::OpenAi,
                    base_url: "http://127.0.0.1:11434".into(),
                    aws_region: None,
                    local: true,
                },
                ResolvedProvider {
                    id: "claudeish".into(),
                    shape: WireShape::Anthropic,
                    base_url: "https://anthropic-gw.example.com".into(),
                    aws_region: None,
                    local: false,
                },
            ],
        }
    }

    fn rules(aliases: &[(&str, &str)], tiers: &[(&str, &str)]) -> RoutingRules {
        RoutingRules {
            aliases: aliases
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            tiers: tiers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn alias_routes_to_registry_provider_and_rewrites_model() {
        let mut body = json!({"model": "acme/fast", "messages": [{"role":"user","content":"hi"}]});
        let d = route_request(
            &mut body,
            "OpenAI",
            &upstreams_with_foundry(),
            &rules(&[("acme/fast", "foundry:gpt-4o-mini")], &[]),
            false,
        )
        .expect("routed");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(d.routed_from, "acme/fast");
        assert_eq!(d.provider_id.as_deref(), Some("foundry"));
        assert_eq!(d.upstream_base.as_deref(), Some("http://127.0.0.1:11434"));
        assert_eq!(d.local, Some(true));
    }

    #[test]
    fn alias_model_only_keeps_upstream() {
        let mut body =
            json!({"model": "claude-opus-4-5", "messages": [{"role":"user","content":"hi"}]});
        let d = route_request(
            &mut body,
            "Anthropic",
            &upstreams_with_foundry(),
            &rules(&[("claude-opus-4-5", "claude-sonnet-4-5")], &[]),
            false,
        )
        .expect("routed");
        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(d.upstream_base, None);
        assert_eq!(d.provider_id, None);
    }

    #[test]
    fn cross_shape_target_is_passthrough_when_xlat_not_allowed() {
        // Anthropic request → OpenAI-shape foundry with xlat_ok=false (wrong
        // path, e.g. count_tokens): must stay passthrough.
        let mut body =
            json!({"model": "claude-opus-4-5", "messages": [{"role":"user","content":"hi"}]});
        let before = body.clone();
        let d = route_request(
            &mut body,
            "Anthropic",
            &upstreams_with_foundry(),
            &rules(&[("claude-opus-4-5", "foundry:gpt-4o-mini")], &[]),
            false,
        );
        assert_eq!(d, None);
        assert_eq!(body, before, "fail-open must leave the body untouched");
    }

    #[cfg(feature = "shape-xlat")]
    #[test]
    fn cross_shape_target_routes_with_translation_flag() {
        // enterprise#16: with the feature compiled in and the caller vouching
        // for the path, Anthropic → OpenAI-shape routes and marks xlat.
        let mut body =
            json!({"model": "claude-opus-4-5", "messages": [{"role":"user","content":"hi"}]});
        let d = route_request(
            &mut body,
            "Anthropic",
            &upstreams_with_foundry(),
            &rules(&[("claude-opus-4-5", "foundry:gpt-4o-mini")], &[]),
            true,
        )
        .expect("cross-shape route with translation");
        assert!(d.xlat, "decision must carry the translation flag");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(d.provider_id.as_deref(), Some("foundry"));

        // Within-shape decisions never set xlat.
        let mut body2 = json!({"model": "acme/fast", "messages": [{"role":"user","content":"hi"}]});
        let d2 = route_request(
            &mut body2,
            "OpenAI",
            &upstreams_with_foundry(),
            &rules(&[("acme/fast", "foundry:gpt-4o-mini")], &[]),
            true,
        )
        .expect("within-shape route");
        assert!(!d2.xlat);
    }

    #[cfg(feature = "shape-xlat")]
    #[test]
    fn cross_shape_needs_local_target() {
        // A non-local OpenAI-shape target cannot be reached with the caller's
        // Anthropic credentials → passthrough.
        let mut upstreams = upstreams_with_foundry();
        upstreams.providers.push(ResolvedProvider {
            id: "openaiish".into(),
            shape: WireShape::OpenAi,
            base_url: "https://oai-compat.example.com".into(),
            aws_region: None,
            local: false,
        });
        let mut body =
            json!({"model": "claude-opus-4-5", "messages": [{"role":"user","content":"hi"}]});
        let before = body.clone();
        let d = route_request(
            &mut body,
            "Anthropic",
            &upstreams,
            &rules(&[("claude-opus-4-5", "openaiish:gpt-4o-mini")], &[]),
            true,
        );
        assert_eq!(d, None);
        assert_eq!(body, before);

        // The same target when local (e.g. Ollama) needs no credential.
        upstreams.providers.last_mut().unwrap().local = true;
        let d = route_request(
            &mut body,
            "Anthropic",
            &upstreams,
            &rules(&[("claude-opus-4-5", "openaiish:llama3.3")], &[]),
            true,
        )
        .expect("local cross-shape target routes");
        assert!(d.xlat);
        assert_eq!(d.local, Some(true));
    }

    #[cfg(feature = "shape-xlat")]
    #[test]
    fn openai_to_anthropic_direction_stays_passthrough() {
        // Only Anthropic→OpenAI is translated; the reverse pair passes through.
        let mut body = json!({"model": "gpt-5.2", "messages": [{"role":"user","content":"hi"}]});
        let d = route_request(
            &mut body,
            "OpenAI",
            &upstreams_with_foundry(),
            &rules(&[("gpt-5.2", "claudeish:claude-sonnet-4-5")], &[]),
            true,
        );
        assert_eq!(d, None);
    }

    #[test]
    fn unknown_provider_and_disabled_rules_are_passthrough() {
        let mut body = json!({"model": "m", "messages": [{"role":"user","content":"hi"}]});
        let before = body.clone();
        assert_eq!(
            route_request(
                &mut body,
                "OpenAI",
                &upstreams_with_foundry(),
                &rules(&[("m", "nope:x")], &[]),
                false,
            ),
            None
        );
        assert_eq!(body, before);
    }

    #[test]
    fn tier_downgrade_routes_simple_queries_to_cheap_model() {
        // An explore-style question lands on a non-premium tier (fast, or
        // standard when the classifier hedges on low confidence). Both map to
        // the cheap target here — this test pins the routing mechanics; tier
        // assignment itself is covered by the intent_engine tests.
        let mut body = json!({
            "model": "gpt-5.2",
            "messages": [
                {"role":"system","content":"be helpful"},
                {"role":"user","content":"where is the config file for the proxy?"}
            ]
        });
        let d = route_request(
            &mut body,
            "OpenAI",
            &upstreams_with_foundry(),
            &rules(
                &[],
                &[("fast", "foundry:phi-4"), ("standard", "foundry:phi-4")],
            ),
            false,
        )
        .expect("non-premium query must route");
        assert_eq!(body["model"], "phi-4");
        assert_eq!(d.routed_from, "gpt-5.2");
        assert_eq!(d.provider_id.as_deref(), Some("foundry"));
    }

    #[test]
    fn premium_tier_unset_keeps_requested_model() {
        // Generation work classifies premium; with no premium target the
        // request passes through untouched.
        let mut body = json!({
            "model": "gpt-5.2",
            "messages": [{"role":"user","content":
                "implement a new distributed lock manager with leader election and fencing tokens"}]
        });
        let before = body.clone();
        let d = route_request(
            &mut body,
            "OpenAI",
            &upstreams_with_foundry(),
            &rules(&[], &[("fast", "foundry:phi-4"), ("premium", "")]),
            false,
        );
        assert_eq!(d, None);
        assert_eq!(body, before);
    }

    #[test]
    fn responses_input_string_and_items_are_extractable() {
        let s = json!({"model":"m","input":"quick question about rust"});
        assert!(extract_user_query(&s, WireShape::OpenAi).is_some());

        let items = json!({"model":"m","input":[
            {"type":"message","role":"user","content":[{"type":"input_text","text":"what does this do"}]}
        ]});
        assert_eq!(
            extract_user_query(&items, WireShape::OpenAi).as_deref(),
            Some("what does this do")
        );

        let anthropic = json!({"model":"m","messages":[
            {"role":"user","content":[{"type":"text","text":"first"}]},
            {"role":"assistant","content":"a"},
            {"role":"user","content":[{"type":"text","text":"latest question"}]}
        ]});
        assert_eq!(
            extract_user_query(&anthropic, WireShape::Anthropic).as_deref(),
            Some("latest question")
        );
    }

    #[test]
    fn missing_model_or_query_is_passthrough() {
        let mut no_model = json!({"messages":[{"role":"user","content":"hi"}]});
        assert_eq!(
            route_request(
                &mut no_model,
                "OpenAI",
                &upstreams_with_foundry(),
                &rules(&[], &[("fast", "foundry:phi-4")]),
                false,
            ),
            None
        );
        let mut no_user = json!({"model":"m","messages":[{"role":"system","content":"x"}]});
        assert_eq!(
            route_request(
                &mut no_user,
                "OpenAI",
                &upstreams_with_foundry(),
                &rules(&[], &[("fast", "foundry:phi-4")]),
                false,
            ),
            None
        );
    }

    #[test]
    fn gemini_and_unknown_labels_are_passthrough() {
        let mut body = json!({"model":"m","messages":[{"role":"user","content":"hi"}]});
        for label in ["Gemini", "Mystery"] {
            assert_eq!(
                route_request(
                    &mut body,
                    label,
                    &upstreams_with_foundry(),
                    &rules(&[("m", "x")], &[]),
                    false,
                ),
                None,
                "{label} must not route in M1"
            );
        }
    }
}
