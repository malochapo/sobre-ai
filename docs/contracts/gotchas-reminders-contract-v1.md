# Gotchas/Reminders Contract v1

**Status**: Stable  
**Version**: 1  
**Runtime source**: `rust/src/core/gotcha_tracker/model.rs`

## Purpose

Formalizes the Gotcha tracking system: structured provenance, category-specific decay, retrieval budgets, and time-bounded reminders.

## Gotcha Schema

```rust
struct Gotcha {
    id: String,
    category: GotchaCategory,       // Build, Test, Config, Runtime, Dependency, Platform, Convention, Security
    severity: GotchaSeverity,        // Critical, Warning, Info
    trigger: String,
    resolution: String,
    file_patterns: Vec<String>,
    occurrences: u32,
    session_ids: Vec<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    confidence: f32,
    source: GotchaSource,
    prevented_count: u32,
    tags: Vec<String>,
    provenance: Vec<ProvenanceRef>,  // NEW v1
    expires_at: Option<DateTime<Utc>>,  // NEW v1
    decay_rate_override: Option<f32>,   // NEW v1
}
```

## ProvenanceRef

Structured origin tracking replacing the previous unstructured `source: String`:

```rust
struct ProvenanceRef {
    kind: String,           // "issue", "commit", "tool_call", "agent", "manual"
    url: Option<String>,    // e.g. "https://gitlab.com/group/project/-/issues/123"
    commit_hash: Option<String>,
    tool_call_id: Option<String>,
    session_id: Option<String>,
}
```

## GotchaPolicy

Configurable via `[memory.gotcha]` in the config file (`MemoryPolicy`):

```toml
[memory.gotcha]
max_gotchas_per_project = 100
retrieval_budget_per_room = 10      # max gotchas returned per category
default_decay_rate = 0.03

[memory.gotcha.category_decay_overrides]
security = 0.005                    # security gotchas decay very slowly
convention = 0.05                   # convention gotchas decay faster
```

## Expiration

- `expires_at`: absolute time after which the gotcha is automatically archived
- `decay_rate_override` per gotcha: overrides the category/default decay rate

## Retrieval Budgets

`retrieval_budget_per_room` limits how many gotchas are returned per category during recall. Prevents one noisy category from dominating context.

## Tool Actions

`ctx_knowledge action=gotcha` records a gotcha for the current project:
`trigger` and `resolution` are required, `severity` and `category` optional
(category defaults to `convention`). Recorded gotchas are recalled through the
knowledge search.
