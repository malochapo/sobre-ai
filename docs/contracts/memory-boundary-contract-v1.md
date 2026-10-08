# Memory Boundary Contract v1

**Status**: Stable  
**Version**: 1  
**Runtime source**: `rust/src/tools/ctx_knowledge/search.rs`, `rust/src/core/session/persistence.rs`, `rust/src/core/gotcha_tracker/persist.rs`

## Purpose

Prevents cross-project memory leakage. Knowledge facts and gotchas are scoped
to their originating project, and a session is resumed only in its own project.
The one deliberate exception is the session part of `ctx_knowledge
action=search`, which searches the findings and decisions of every saved
session and labels each hit with its project.

## Enforcement points

| Tool / action | Behaviour |
|---|---|
| `ctx_knowledge action=search` | Knowledge facts: current project only. Session findings and decisions: all saved sessions, each labeled with its project |
| `ctx_knowledge action=recall` | Loads from the current project only |
| Session `load_latest` | Scoped to the project root; no global fallback |
| Universal gotchas | Shared across projects by design; `universal-gotchas.json` in the data directory is always loaded |
