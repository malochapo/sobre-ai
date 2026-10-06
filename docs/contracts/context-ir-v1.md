# Context IR v1 (ContextIrV1)

GitLab: `#2308`

LeanCTX produces many kinds of context (read/search/shell/provider). Context IR v1 is the **stable, versioned intermediate representation** that captures:

- **Content** (bounded excerpt)
- **Tokens** (input/output + saved)
- **Provenance** (tool + optional path/command/pattern, redaction-safe)
- **Safety** (redaction applied + boundary-mode hint)
- **Verification** (content checksum)
- **Latency / metrics** (duration + compression ratio)

## Stability & versioning

- Payload includes `schema_version` (SSOT: `rust/src/core/contracts.rs`).
- Additive changes must remain backwards-compatible for readers.

## Bounds / DoS safety

Context IR is an **observability artifact**. It is intentionally bounded:

- Max items per store: `128`
- Max excerpt per item: `4096` chars
- Max total excerpt chars across items: `65536` chars

Older items are pruned first when limits are exceeded.

## Redaction / secrets

- Stored fields are redacted using `rust/src/core/redaction.rs` and are safe to persist.
- Provenance fields (`command`, `pattern`) are redacted and bounded via excerpt limits.

## Storage

- Runtime store (local): `context_ir_v1.json` in the cache directory
  (`rust/src/core/paths.rs` `cache_dir()`).
- Read back by the context snapshot builder (`rust/src/core/context_snapshot/builder.rs`).

## Relevant code

- Schema + store: `rust/src/core/context_ir.rs`
- Collection points:
  - `rust/src/server/call_tool.rs` (every MCP tool call)
  - `rust/src/core/tool_lifecycle.rs`
