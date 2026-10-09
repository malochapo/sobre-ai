# CCP Session Bundle v1 (CcpSessionBundleV1)

CCP session bundles standardise **export/import** of session excerpts for
replayability: task, findings, decisions and touched files.

## Goals

- **Deterministic**: the same session gives the same bundle (except `exported_at`).
- **Bounded**: fixed caps (counts and bytes).
- **Replayable**: carries the hash of the project's canonical root.
- **Redacted**: exports never contain secret-like content.

## Bundle format (JSON)

Top-level fields:

- `schema_version`: `1`
- `exported_at`: RFC3339 timestamp
- `project`: `project_root_hash`, the key every store uses for the project
- `session`: bounded excerpt (task, findings, decisions, files, stats,
  `terse_mode`, `compression_level`). The last two record the exporting
  machine's `compression_level` setting; importing a bundle never applies them.

## Privacy

Exports are always redacted: strings go through `redaction::redact_text`,
and the project is named by its hash, never its path. There is no unredacted
export.

## Where a bundle is written

`sobre_session action=export` returns the bundle. With `path=` it also writes
it there; with `write=true` and no `path`, under
`<data_dir>/exports/sessions/`. Nothing is written into the project
unless `path=` points there.

## Import semantics

- Import sets the session excerpt fields (bounded) and marks missing or stale
  file paths.
- A bundle from another project root is reported as a warning; import still
  proceeds (provenance is kept). Paths are read against the importing
  session's root.

## Boundedness

- The session itself is bounded by its `MAX_*` caps (`rust/src/core/session/`).
- Export and import also enforce a maximum JSON size of 250 000 bytes.

## Surface

Tool:

- `sobre_session action=export format=json|summary write=true|false path=<optional>`
- `sobre_session action=import path=<bundle.json>`

## Relevant code

- Bundle contract/runtime: `rust/src/core/ccp_session_bundle.rs`
- Session store: `rust/src/core/session/`
- Tool surface: `rust/src/tools/sobre_session.rs` (registered in
  `rust/src/tools/registered/sobre_session.rs`)
