# Memory Boundary Contract v1

**Status**: Stable  
**Version**: 1  
**Runtime source**: `rust/src/tools/ctx_knowledge/search.rs`, `rust/src/core/session/persistence.rs`, `rust/src/core/gotcha_tracker/persist.rs`

## Purpose

Prevents cross-project memory leakage. Knowledge facts, sessions and gotchas
are scoped to their originating project, and nothing reads another project's
memory. The project is the session's project root, never the server process's
working directory.

## Enforcement points

| Tool / action | Behaviour |
|---|---|
| `ctx_knowledge action=search` | The current project's knowledge facts, and the findings and decisions of its saved sessions |
| `ctx_knowledge action=recall` | Loads from the current project only |
| Session `load_latest` | Scoped to the project root; no global fallback |
