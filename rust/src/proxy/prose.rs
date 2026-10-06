//! Prompt-cache anchors in request bodies: a `cache_control` breakpoint marks
//! content the client's prompt cache depends on, so it is never rewritten.

use serde_json::Value;

/// `true` if a content block carries a `cache_control` breakpoint — such a
/// block anchors the client's prompt cache and must never be rewritten.
fn block_has_cache_control(block: &Value) -> bool {
    block.get("cache_control").is_some()
}

/// `true` if a `system` value (string or array of blocks) carries any
/// `cache_control` breakpoint. Then it anchors the client's prompt cache and
/// the whole field must be left verbatim. A plain string system prompt never
/// carries one (Anthropic places `cache_control` on blocks), so it is safe.
#[must_use]
pub(crate) fn value_has_cache_control(v: &Value) -> bool {
    match v {
        Value::Array(blocks) => blocks.iter().any(block_has_cache_control),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cache_control_on_value_and_blocks() {
        assert!(!value_has_cache_control(&Value::String("x".into())));
        let anchored = serde_json::json!([{"type": "text", "text": "x", "cache_control": {"type": "ephemeral"}}]);
        assert!(value_has_cache_control(&anchored));
    }
}
