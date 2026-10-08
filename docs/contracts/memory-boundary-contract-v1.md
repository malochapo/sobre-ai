# Memory Boundary Contract v1

**Status**: Stable  
**Version**: 1  
**Runtime source**: `rust/src/tools/ctx_knowledge/search.rs`, `rust/src/core/session/persistence.rs`, `rust/src/core/gotcha_tracker/persist.rs`

## Purpose

Prevents cross-project memory leakage. Knowledge facts, sessions and gotchas
are scoped to their originating project, and nothing reads another project's
memory.

## Enforcement points

| Tool / action | Behaviour |
|---|---|
| `ctx_knowledge action=search` | Scoped to the current project hash; other projects' knowledge is skipped |
| `ctx_knowledge action=recall` | Loads from the current project only |
| Session `load_latest` | Scoped to the project root; no global fallback |
| Universal gotchas | Shared across projects by design; `universal-gotchas.json` in the data directory is always loaded |
