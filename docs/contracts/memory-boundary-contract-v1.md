# Memory Boundary Contract v1

**Status**: Stable  
**Version**: 1  
**Runtime source**: `rust/src/tools/sobre_knowledge/search.rs`, `rust/src/core/session/persistence.rs`, `rust/src/core/gotcha_tracker/persist.rs`

## Purpose

Prevents cross-project memory leakage. Knowledge facts, sessions and gotchas
are scoped to their originating project. The project is the session's project
root, never the server process's working directory.

## Enforcement points

| Tool / action | Behaviour |
|---|---|
| `sobre_knowledge action=search` | The current project's knowledge facts, and the findings and decisions of its saved sessions |
| `sobre_knowledge action=recall` | The current project's facts. Known gap: when none match, facts are brought back from the archive under `memory/archive/`, which every project shares (`rust/src/tools/sobre_knowledge/remember.rs`, `rehydrate_from_archives`) |
| `sobre_session action=load` without `session_id` | This project's latest saved session; no global fallback |
| `sobre_session action=load session_id=…` | The session named, whatever its project |
