//! The usage ledger: one append-only file, `<data_dir>/usage.jsonl`, that every
//! path writes to and `gain` reads.
//!
//! Each line is one OpenTelemetry log record in OTLP/JSON form, with the GenAI
//! semantic-convention attribute names where one exists
//! (`gen_ai.usage.input_tokens`, `gen_ai.request.model`, …) and `sobre.*`
//! names for the rest. Nothing exports it; the format means an OpenTelemetry
//! exporter could read it as is.
//!
//! Three events:
//!
//! - [`EVENT_COMPRESSION`]: one compression on any path (MCP tool, `-c` shell
//!   wrapper, host hook, CLI command, proxy), as tokens before and after, both
//!   counted by [`crate::core::tokens::count_tokens`].
//! - [`EVENT_LLM_CALL`]: one upstream LLM call the proxy forwarded, with the
//!   usage the provider reported.
//! - [`EVENT_BOUNCE`]: a compressed read that was not enough, so the agent
//!   read the file again in full.
//!
//! Nothing here is aggregated on write: [`summarize`] computes every figure from
//! the file when it is read.

mod otlp;
mod summary;

use std::io::Write;
use std::path::PathBuf;

pub use otlp::{Event, Value};
pub use summary::{Period, Summary, summarize};

pub const EVENT_COMPRESSION: &str = "sobre.compression";
pub const EVENT_LLM_CALL: &str = "gen_ai.client.inference.operation.details";
pub const EVENT_BOUNCE: &str = "sobre.bounce";

const FILE_NAME: &str = "usage.jsonl";

/// Where a compression happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// An MCP tool call (`sobre_read`, `sobre_shell`, …).
    Mcp,
    /// The `sobre -c` shell wrapper.
    Shell,
    /// A host hook (post-tool compression, read dedup).
    Hook,
    /// The API proxy, on tool results inside a request.
    Proxy,
    /// `sobre call`: one MCP tool run from the command line.
    Call,
}

impl Surface {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Shell => "shell",
            Self::Hook => "hook",
            Self::Proxy => "proxy",
            Self::Call => "call",
        }
    }
}

/// One upstream LLM call, as the provider reported it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmCall {
    /// `anthropic` or `openai` (GenAI `gen_ai.provider.name`).
    pub provider: String,
    /// The model the client asked for.
    pub request_model: String,
    /// The model that answered.
    pub response_model: String,
    /// All input tokens, cache reads and writes included, as the GenAI
    /// conventions define `gen_ai.usage.input_tokens`.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// The upstream is a loopback or local endpoint: no bill to price.
    pub local: bool,
}

/// Path of the ledger, creating the data directory if needed.
#[must_use]
pub fn path() -> Option<PathBuf> {
    let dir = crate::core::data_dir::sobre_data_dir().ok()?;
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
    if call.local {
        ev.attr("sobre.upstream.local", Value::Bool(true));
    }
    append(&ev);
}

/// Records a compressed read of `sent` out of `original` tokens that turned
/// out to be too little: the agent read the whole file again.
pub fn record_bounce(tool: &str, sent: usize, original: usize) {
    if sent == 0 && original == 0 {
        return;
    }
    let mut ev = Event::now(EVENT_BOUNCE);
    ev.attr("sobre.tool", Value::Str(tool.to_string()));
    ev.attr("sobre.tokens.wasted", Value::Int(sent as u64));
    ev.attr("sobre.tokens.original", Value::Int(original as u64));
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
    // `read` too: on Windows, locking needs a handle with read or write
    // access, and an append-only handle has neither, so the lock failed and
    // nothing was recorded.
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if file.lock().is_ok() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_the_file() {
        let dir = crate::core::data_dir::isolated_data_dir();
        record_compression(Surface::Mcp, "round-trip-read", 1000, 250);
        record_compression(Surface::Shell, "round-trip-shell", 0, 0);
        record_llm_call(&LlmCall {
            provider: "anthropic".into(),
            request_model: "round-trip-model".into(),
            response_model: "round-trip-model".into(),
            input_tokens: 5000,
            output_tokens: 200,
            cache_read_tokens: 4000,
            cache_write_tokens: 0,
            local: false,
        });
        record_bounce("round-trip-read", 300, 1200);

        // Tests elsewhere in this process record through the global bounce
        // tracker while this one holds the data dir, so keep only these events.
        let ours = |e: &Event| {
            e.str("sobre.tool")
                .or_else(|| e.str("gen_ai.request.model"))
                .is_some_and(|t| t.starts_with("round-trip-"))
        };
        let events: Vec<Event> = load().into_iter().filter(ours).collect();
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
        assert_eq!(events[2].int("sobre.tokens.original"), Some(1200));

        assert!(reset());
        assert!(!load().iter().any(ours));
        drop(dir);
    }
}
