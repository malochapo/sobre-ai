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

The `[boundary_policy]` config section has one key:

```rust
struct BoundaryPolicy {
    universal_gotchas_enabled: bool,  // default: true
}
```

When `universal_gotchas_enabled` is false, only project-scoped gotchas are
loaded (`universal-gotchas.json` is skipped).

## Enforcement points

| Tool / action | Behaviour |
|---|---|
| `ctx_knowledge action=search` | Scoped to the current project hash; other projects' knowledge is skipped |
| `ctx_knowledge action=recall` | Loads from the current project only |
| Session `load_latest` | Scoped to the project root; no global fallback |
| Universal gotchas | Shared by design, gated by `universal_gotchas_enabled` |

## Migration

`KnowledgeFact` entries without a `privacy` field default to `ProjectOnly` via
`#[serde(default)]`.
