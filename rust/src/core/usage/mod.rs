//! The usage ledger: one append-only file, `<data_dir>/usage.jsonl`, that every
//! path writes to and `gain` reads.
//!
//! Each line is one OpenTelemetry log record in OTLP/JSON form, with the GenAI
//! semantic-convention attribute names where one exists
//! (`gen_ai.usage.input_tokens`, `gen_ai.request.model`, …) and `sobre.*`
//! names for the rest. Nothing exports it; the format only means an exporter
//! can be added later without a migration.
//!
//! Three events:
//!
//! - [`EVENT_COMPRESSION`]: one compression on any path (MCP tool, `-c` shell
//!   wrapper, host hook, CLI command, proxy), as tokens before and after, both
//!   counted by [`crate::core::tokens::count_tokens`].
//! - [`EVENT_LLM_CALL`]: one upstream LLM call the proxy forwarded, with the
//!   usage the provider reported and, when it reports one, the cost it charged.
//! - [`EVENT_BOUNCE`]: tokens re-sent because a compressed read was not enough
//!   and the agent read the file again in full.
//!
//! Nothing here is aggregated on write: [`summarize`] computes every figure from
//! the file when it is read.

mod otlp;
mod summary;

use std::io::Write;
use std::path::PathBuf;

pub use otlp::{Event, Value};
pub use summary::{Period, Summary, ToolTotals, summarize};

pub const EVENT_COMPRESSION: &str = "sobre.compression";
pub const EVENT_LLM_CALL: &str = "gen_ai.client.inference.operation.details";
pub const EVENT_BOUNCE: &str = "sobre.bounce";

const FILE_NAME: &str = "usage.jsonl";

/// Where a compression happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// An MCP tool call (`ctx_read`, `ctx_shell`, …).
    Mcp,
    /// The `lean-ctx -c` shell wrapper.
    Shell,
    /// A host hook (post-tool compression, read dedup).
    Hook,
    /// A CLI command (`lean-ctx read`, `diff`, …).
    Cli,
    /// The API proxy, on tool results inside a request.
    Proxy,
}

impl Surface {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Shell => "shell",
            Self::Hook => "hook",
            Self::Cli => "cli",
            Self::Proxy => "proxy",
        }
    }
}

/// One upstream LLM call, as the provider reported it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmCall {
    /// `anthropic`, `openai`, `gcp.gemini`, … (GenAI `gen_ai.provider.name`).
    pub provider: String,
    /// The model the client asked for.
    pub request_model: String,
    /// The model that answered (differs when the proxy routed the call).
    pub response_model: String,
    /// All input tokens, cache reads and writes included, as the GenAI
    /// conventions define `gen_ai.usage.input_tokens`.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// What the provider charged, when the response says (OpenRouter, LiteLLM).
    pub cost_usd: Option<f64>,
    /// The upstream is a loopback or local endpoint: no bill to price.
    pub local: bool,
}

/// Path of the ledger, creating the data directory if needed.
#[must_use]
pub fn path() -> Option<PathBuf> {
    let dir = crate::core::data_dir::lean_ctx_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(FILE_NAME))
}

/// Records one compression. `original` and `sent` are token counts; an event
/// with nothing to compress (`original == 0`) is not recorded.
pub fn record_compression(surface: Surface, tool: &str, original: usize, sent: usize) {
    if original == 0 {
        return;
    }
    let mut ev = Event::now(EVENT_COMPRESSION);
    ev.attr("sobre.surface", Value::Str(surface.as_str().to_string()));
    ev.attr("sobre.tool", Value::Str(tool.to_string()));
    ev.attr("sobre.tokens.original", Value::Int(original as u64));
    ev.attr("sobre.tokens.sent", Value::Int(sent as u64));
    append(&ev);
}

/// Records one upstream LLM call.
pub fn record_llm_call(call: &LlmCall) {
    let mut ev = Event::now(EVENT_LLM_CALL);
    ev.attr("gen_ai.operation.name", Value::Str("chat".to_string()));
    ev.attr("gen_ai.provider.name", Value::Str(call.provider.clone()));
    ev.attr(
        "gen_ai.request.model",
        Value::Str(call.request_model.clone()),
    );
    ev.attr(
        "gen_ai.response.model",
        Value::Str(call.response_model.clone()),
    );
    ev.attr("gen_ai.usage.input_tokens", Value::Int(call.input_tokens));
    ev.attr("gen_ai.usage.output_tokens", Value::Int(call.output_tokens));
    ev.attr(
        "gen_ai.usage.cache_read.input_tokens",
        Value::Int(call.cache_read_tokens),
    );
    ev.attr(
        "gen_ai.usage.cache_creation.input_tokens",
        Value::Int(call.cache_write_tokens),
    );
    if let Some(usd) = call.cost_usd {
        ev.attr("sobre.cost.usd", Value::Double(usd));
    }
    if call.local {
        ev.attr("sobre.upstream.local", Value::Bool(true));
    }
    append(&ev);
}

/// Records tokens re-sent after a compressed read turned out to be too little.
pub fn record_bounce(tool: &str, wasted: usize) {
    if wasted == 0 {
        return;
    }
    let mut ev = Event::now(EVENT_BOUNCE);
    ev.attr("sobre.tool", Value::Str(tool.to_string()));
    ev.attr("sobre.tokens.wasted", Value::Int(wasted as u64));
    append(&ev);
}

/// Reads every event in the ledger, skipping lines that do not parse.
#[must_use]
pub fn load() -> Vec<Event> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(Event::from_json_line).collect()
}

/// Deletes the ledger. Returns whether a file was removed.
pub fn reset() -> bool {
    path().is_some_and(|p| std::fs::remove_file(p).is_ok())
}

/// Appends one line. Usage recording never fails the call that triggered it,
/// so errors are dropped. One `write` of a line opened in append mode lands
/// whole even with several writers (the MCP server, hooks and the proxy are
/// separate processes); the advisory lock keeps that true for long lines too.
fn append(ev: &Event) {
    let Some(path) = path() else {
        return;
    };
    let mut line = ev.to_json_line();
    line.push('\n');
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use fs2::FileExt;
    if file.lock_exclusive().is_ok() {
        let _ = file.write_all(line.as_bytes());
        let _ = FileExt::unlock(&file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_the_file() {
        let dir = crate::core::data_dir::isolated_data_dir();
        record_compression(Surface::Mcp, "ctx_read", 1000, 250);
        record_compression(Surface::Shell, "cargo test", 0, 0);
        record_llm_call(&LlmCall {
            provider: "anthropic".into(),
            request_model: "claude-sonnet-4-5".into(),
            response_model: "claude-sonnet-4-5".into(),
            input_tokens: 5000,
            output_tokens: 200,
            cache_read_tokens: 4000,
            cache_write_tokens: 0,
            cost_usd: None,
            local: false,
        });
        record_bounce("ctx_read", 300);

        let events = load();
        assert_eq!(events.len(), 3, "the empty compression is not recorded");
        assert_eq!(events[0].name, EVENT_COMPRESSION);
        assert_eq!(events[0].int("sobre.tokens.original"), Some(1000));
        assert_eq!(events[0].str("sobre.surface"), Some("mcp"));
        assert_eq!(events[1].name, EVENT_LLM_CALL);
        assert_eq!(
            events[1].int("gen_ai.usage.cache_read.input_tokens"),
            Some(4000)
        );
        assert_eq!(events[2].int("sobre.tokens.wasted"), Some(300));

        assert!(reset());
        assert!(load().is_empty());
        drop(dir);
    }
}
