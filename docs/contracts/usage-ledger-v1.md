# Usage ledger v1

The usage ledger is the one record of what sobre compressed and, when the proxy
is in the path, of what each LLM call cost. Every path writes to it; `sobre
gain` reads it. Nothing is aggregated on write: every total is computed from
the file when it is read.

## Location

`<data_dir>/usage.jsonl`, where `<data_dir>` is `SOBRE_DATA_DIR` when set,
else `$XDG_DATA_HOME/sobre` (default `~/.local/share/sobre`).

Append-only, one record per line. Each append is a single write under an
advisory file lock, so the MCP server, hooks, CLI commands and the proxy (all
separate processes) can write concurrently. A reader skips any line it cannot
parse, such as a line cut short by a crash.

`sobre gain --reset` deletes the file.

## Record format

Each line is one OpenTelemetry log record in the OTLP/JSON encoding:

```json
{
  "timeUnixNano": "1791384000000000000",
  "eventName": "sobre.compression",
  "attributes": [
    { "key": "sobre.surface", "value": { "stringValue": "mcp" } },
    { "key": "sobre.tool", "value": { "stringValue": "sobre_read" } },
    { "key": "sobre.tokens.original", "value": { "intValue": "4210" } },
    { "key": "sobre.tokens.sent", "value": { "intValue": "612" } }
  ]
}
```

- `timeUnixNano` and every `intValue` are decimal strings, as OTLP/JSON
  encodes 64-bit integers.
- Values use `stringValue`, `intValue` or `boolValue`.
- Attribute names follow the OpenTelemetry GenAI semantic conventions where one
  exists; the others are under `sobre.*`.

Nothing exports the file today. The format exists so an exporter can be added
without a migration.

## Events

### `sobre.compression`

One compression on any path. Token counts use one tokenizer everywhere
(`o200k_base`), so every path is in the same unit. A compression with nothing
to compress (`original = 0`) is not recorded.

| Attribute | Type | Meaning |
|---|---|---|
| `sobre.surface` | string | `mcp`, `shell` (`sobre -c`), `hook`, `proxy` or `call` (`sobre call`) |
| `sobre.tool` | string | MCP tool name, shell program name (`cargo`, `git`), hook name, or `request` for the proxy |
| `sobre.tokens.original` | int | Tokens before compression |
| `sobre.tokens.sent` | int | Tokens actually sent to the model |

On the MCP path the record is written once per call, after all
post-processing, so `sent` is what the model receives.

### `gen_ai.client.inference.operation.details`

One upstream LLM call the proxy forwarded, as the provider reported it.

| Attribute | Type | Meaning |
|---|---|---|
| `gen_ai.operation.name` | string | `chat` |
| `gen_ai.provider.name` | string | `anthropic` or `openai` |
| `gen_ai.request.model` | string | Model the client asked for |
| `gen_ai.response.model` | string | Model that answered |
| `gen_ai.usage.input_tokens` | int | All input tokens, cache reads and writes included |
| `gen_ai.usage.output_tokens` | int | Output tokens, reasoning included |
| `gen_ai.usage.cache_read.input_tokens` | int | Input tokens served from the prompt cache |
| `gen_ai.usage.cache_creation.input_tokens` | int | Input tokens written to the prompt cache |
| `sobre.upstream.local` | bool | Present and `true` when the upstream is a local endpoint |

### `sobre.bounce`

Tokens re-sent because a compressed read was not enough and the agent read the
file again in full.

| Attribute | Type | Meaning |
|---|---|---|
| `sobre.tool` | string | `read` |
| `sobre.tokens.wasted` | int | Tokens of the compressed read that turned out useless |

## Derived figures

`gain` computes these at read time:

- **Net saved tokens**: the sum of `original − sent` over compressions, minus
  the sum of `wasted` over bounces.
- **Real cost**: the price table applied to the tokens each LLM call
  reported, `0` for a call to a local upstream.
  Without the proxy there are no calls and no real cost.
- **Saved value**: net saved tokens at the uncached input price of a model:
  the one passed with `--model`, else the model that answered the most calls,
  else the model a host hook last reported, else a blended fallback. It is an
  estimate, not a bound: a removed tool result would have been re-sent on
  later turns, mostly as cheaper cache reads, and the proxy re-compresses the
  whole history on every request, so a result it shortened counts once per
  request that carried it. When every call went to a local upstream, `gain`
  shows no money figure.
- **Cost without sobre**: real cost plus saved value.

## Stability

This file is frozen: any change to it, an added attribute included, goes in a
new `usage-ledger-v2.md`. Readers ignore attributes and events they do not
know, so records written under v2 stay readable by v1 code.

## Relevant code

- Writer, reader and OTLP encoding: `rust/src/core/usage/`
- Totals: `rust/src/core/usage/summary.rs`
- Proxy calls: `rust/src/proxy/usage_meter.rs`
- Report: `rust/src/cli/dispatch/analytics/gain.rs`
