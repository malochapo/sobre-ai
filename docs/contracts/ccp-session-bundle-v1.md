# CCP Session Bundle v1 (CcpSessionBundleV1)

GitLab: `#2316`

CCP session bundles standardise **export/import** of session excerpts for
replayability: task, findings, decisions, evidence (tool receipts and manual
keys).

## Goals

- **Deterministic**: the same session gives the same bundle (except `exported_at`).
- **Bounded**: fixed caps (counts and bytes).
- **Replayable**: carries the project identity hash.
- **Redacted**: exports never contain secret-like content.

## Bundle format (JSON)

Top-level fields:

- `schema_version`: `1`
- `exported_at`: RFC3339 timestamp
- `project`: project hashes (root + identity)
- `session`: bounded excerpt (task, findings, decisions, files, progress,
  next steps, evidence, stats, `terse_mode`, `compression_level`). The last
  two record the exporting machine's `compression_level` setting; importing a
  bundle never applies them.

## Privacy

Exports are always redacted: evidence `value`s are dropped and strings go
through `redaction::redact_text`. `privacy=full` is refused.

## Import semantics

- Import sets the session excerpt fields (bounded) and marks missing or stale
  file paths.
- A project identity mismatch is reported as a warning; import still proceeds
  (provenance is kept).

## Boundedness

- The session itself is bounded by its `MAX_*` caps (`rust/src/core/session/`).
- Export and import also enforce a maximum JSON size of 250 000 bytes.

## Surface

Tool:

- `ctx_session action=export format=json|summary write=true|false path=<optional>`
- `ctx_session action=import path=<bundle.json>`

## Relevant code

- Bundle contract/runtime: `rust/src/core/ccp_session_bundle.rs`
- Session store: `rust/src/core/session/`
- Tool surface: `rust/src/tools/ctx_session.rs` (registered in
  `rust/src/tools/registered/ctx_session.rs`)
