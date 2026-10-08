# Memory Boundary Contract v1

**Status**: Stable  
**Version**: 1  
**Runtime source**: `rust/src/core/memory_boundary.rs`

## Purpose

Prevents cross-project memory leakage. Knowledge facts, sessions and gotchas
are scoped to their originating project, and nothing reads another project's
memory.

## FactPrivacy

Every `KnowledgeFact` carries a `privacy` field:

```rust
enum FactPrivacy {
    ProjectOnly,    // default — only visible within the originating project
    LinkedProjects,
}
```

Facts are created as `ProjectOnly`.

## BoundaryPolicy

`BoundaryPolicy` (`rust/src/core/memory_boundary.rs`) has one field:

```rust
struct BoundaryPolicy {
    universal_gotchas_enabled: bool,  // default: true
}
```

There is no config section for it. `load_universal_gotchas`
(`rust/src/core/gotcha_tracker/persist.rs`) uses `BoundaryPolicy::default()`,
so universal gotchas (`universal-gotchas.json` in the data directory) are
always loaded. The `false` branch, which skips that file and uses only
project-scoped gotchas, is not reachable from configuration.

## Enforcement points

| Tool / action | Behaviour |
|---|---|
| `ctx_knowledge action=search` | Scoped to the current project hash; other projects' knowledge is skipped |
| `ctx_knowledge action=recall` | Loads from the current project only |
| Session `load_latest` | Scoped to the project root; no global fallback |
| Universal gotchas | Shared across projects by design; always loaded (`BoundaryPolicy::default()`) |
