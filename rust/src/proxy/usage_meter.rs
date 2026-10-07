//! Hands each measured LLM call to the usage ledger.
//!
//! [`super::usage`] parses what the provider reported; this module maps it onto
//! a [`crate::core::usage::LlmCall`]. Pricing happens when the ledger is read.

/// Records one upstream call the proxy forwarded.
pub(crate) fn record(u: &super::usage::RealUsage) {
    let wire = u.wire.as_deref();
    crate::core::usage::record_llm_call(&crate::core::usage::LlmCall {
        provider: wire.map_or_else(String::new, |w| provider_name(&w.provider)),
        request_model: wire
            .and_then(|w| w.routed_from.clone())
            .unwrap_or_else(|| u.model.clone()),
        response_model: u.model.clone(),
        // The GenAI conventions count cache reads and writes inside input.
        input_tokens: u.input_tokens + u.cache_read_tokens + u.cache_write_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens: u.cache_read_tokens,
        cache_write_tokens: u.cache_write_tokens,
        cost_usd: u.provider_cost_usd,
        local: wire.is_some_and(|w| w.is_local),
    });
}

/// The proxy's provider label as a GenAI `gen_ai.provider.name` value. Registry
/// providers keep their own id.
fn provider_name(label: &str) -> String {
    match label {
        "Anthropic" => "anthropic".to_string(),
        "OpenAI" => "openai".to_string(),
        "Azure" => "azure.ai.openai".to_string(),
        "Bedrock" => "aws.bedrock".to_string(),
        "Gemini" | "Google" => "gcp.gemini".to_string(),
        other => other.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_labels_map_to_semconv_names() {
        assert_eq!(provider_name("Anthropic"), "anthropic");
        assert_eq!(provider_name("Bedrock"), "aws.bedrock");
        assert_eq!(provider_name("foundry"), "foundry");
    }
}
