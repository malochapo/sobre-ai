# Gotchas/Reminders Contract v1

**Status**: Stable  
**Version**: 1  
**Runtime source**: `rust/src/core/gotcha_tracker/model.rs`

## Purpose

Formalizes the Gotcha tracking system: the gotcha schema, the store cap, and source-based confidence decay.

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
}
```

## GotchaPolicy

`GotchaPolicy` (`rust/src/core/memory_policy.rs`) is part of `MemoryPolicy`.
There is no config-file section for it: callers use `MemoryPolicy::default()`,
so the values are fixed:

| Field | Value |
|---|---|
| `max_gotchas_per_project` | 100 |

It is read by the doctor storage check. The store enforces its own constants
in `rust/src/core/gotcha_tracker/model.rs`.

## Store limits and decay

Fixed constants in `gotcha_tracker/model.rs` and `gotcha_tracker/mod.rs`:

- **Cap**: at most 100 gotchas per project (`MAX_GOTCHAS`). When a new gotcha
  pushes the store over the cap, gotchas are sorted by confidence and the
  lowest-confidence ones are dropped.
- **Decay** (`GotchaStore::apply_decay`, run when the store is loaded): for a
  gotcha not seen for at least one day, confidence drops by
  `source_rate * 1 / (1 + 0.1 * occurrences) * days_since_last_seen / 7`.
  The source rate is 0.02 (agent-reported), 0.03 (cross-session correlated)
  or 0.05 (auto-detected).
- **Archive threshold**: gotchas whose confidence falls below 0.15 are
  removed from the store.
- **Pending errors**: at most 10, each expiring after 15 minutes.

## Tool Actions

`sobre_knowledge action=gotcha` records a gotcha for the current project:
`trigger` and `resolution` are required, `severity` and `category` optional
(category defaults to `convention`). Recorded gotchas are recalled through the
knowledge search.
