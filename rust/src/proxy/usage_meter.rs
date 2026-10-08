//! Hands each measured LLM call to the usage ledger.
//!
//! [`super::usage`] parses what the provider reported; this module maps it onto
//! a [`crate::core::usage::LlmCall`]. Pricing happens when the ledger is read.

/// Records one upstream call the proxy forwarded.
pub(crate) fn record(u: &super::usage::RealUsage) {
    let wire = u.wire.as_deref();
    crate::core::usage::record_llm_call(&crate::core::usage::LlmCall {
        provider: wire.map_or("", |w| w.provider.semconv_name()).to_string(),
        request_model: wire
            .and_then(|w| w.request_model.clone())
            .unwrap_or_else(|| u.model.clone()),
        response_model: u.model.clone(),
        // The GenAI conventions count cache reads and writes inside input.
        input_tokens: u.input_tokens + u.cache_read_tokens + u.cache_write_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens: u.cache_read_tokens,
        cache_write_tokens: u.cache_write_tokens,
        local: wire.is_some_and(|w| w.is_local),
    });
}
