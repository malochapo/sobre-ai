# Knowledge Policy Contract v1

GitLab: `#2317`

## Purpose

A versioned governance contract for project knowledge (facts, patterns,
history, relations) that is:

- **bounded**: stores and outputs have fixed caps,
- **deterministic**: stable ordering and stable semantics,
- **replayable**: the fact timeline stays interpretable,
- **safe**: no secret leakage; surfaces are policy-gated.

## Version

- v1 (this document). The policy data carries no `schema_version` of its own.

## Policy source

The policy is `MemoryPolicy` (`rust/src/core/memory_policy.rs`). Every caller
uses `MemoryPolicy::default()`: there is no config-file section and no
environment override for it, so the values below are fixed.
`MemoryPolicy::validate()` defines the valid value ranges; it is called only
from unit tests, which assert that the defaults are valid.

### Knowledge (`KnowledgePolicy`)

| Field | Value |
|---|---|
| `max_facts` | 200 |
| `max_patterns` | 50 |
| `max_history` | 100 |
| `contradiction_threshold` | 0.5 |
| `recall_facts_limit` | 10 (`core/budgets.rs`) |
| `rooms_limit` | 25 (`core/budgets.rs`) |
| `timeline_limit` | 25 (`core/budgets.rs`) |
| `relations_limit` | 40 |

### Lifecycle (`LifecyclePolicy`)

| Field | Value |
|---|---|
| `low_confidence_threshold` | 0.3 |
| `stale_days` | 30 |
| `similarity_threshold` | 0.85 |
| `base_stability_days` | 90.0 |
| `archetype_aware_decay` | false |
| `prune_unretrieved_after_days` | 90 |
| `reclaim_headroom_pct` | 0.25 (a full store settles at 75%) |

`MemoryPolicy` also holds embeddings, gotcha and admission sub-policies;
their defaults are in the same file. None of it is configurable.

## Semantics (MUST)

### Facts

- A fact is addressed by `(category, key)`.
- Current vs archived:
  - current: `valid_until == None`
  - archived: `valid_until != None`
- An update must not lose the previous state: the timeline returns the
  archived versions plus the current one.

### Contradictions

Checked against the current fact with the same `(category, key)`
(`core/knowledge/core.rs`, `check_contradiction`):

- Case-insensitive equal values ⇒ **no** contradiction.
- Word similarity above 0.8 ⇒ **no** contradiction (avoids false positives
  on semantically equal values).
- Severity:
  - `High`: `existing.confidence >= 0.9` **and** `existing.confirmation_count >= 2`
  - `Medium`: `existing.confidence >= contradiction_threshold`
  - `Low`: otherwise

### Retrieval budgets (bounded, deterministic)

- `recall_facts_limit`: max facts in recall outputs
- `rooms_limit`: max rooms in `ctx_knowledge action=rooms`
- `timeline_limit`: max entries in `ctx_knowledge action=timeline`
- `relations_limit`: max edges in `ctx_knowledge action=relations|relations_diagram`

Results are sorted with stable tie-breaks, then truncated.

### Lifecycle

Confidence decay (an Ebbinghaus curve), consolidation and compaction are
deterministic and bounded (`rust/src/core/memory_lifecycle.rs`), parameterised
by `base_stability_days`, `low_confidence_threshold`, `stale_days`,
`similarity_threshold` and `max_facts`.

### Lossless eviction (MUST, #995)

- **No hard drop.** Every store (facts, history, patterns) evicts
  only through the capacity manager
  (`rust/src/core/memory_capacity.rs::reclaim_store`). The evicted tail is
  archived **before** removal and can be restored. Facts are archived at
  `memory/archive/archive-*.json`; the other stores under
  `memory/archive/<store>/<project-hash>/archive-*.json`
  (`rust/src/core/memory_archive.rs`).
- **One formula plus hysteresis.** `reclaim_target(max, reclaim_headroom_pct)`
  (`max - ceil(max * pct)`, with `pct` clamped to `[0.0, 0.95]`) is the only
  target-size calculation. A reclaim triggers only when `len >= max` and then
  settles the store at `reclaim_target`, so there is no churn on every write.
- **Retention ranking** per store is deterministic (stable sort tie-breaks):
  facts by `is_current` then output ranking, history and patterns newest first.
- **Archive reachability.** `ArchiveConfig::default()` sets both `max_files`
  and `rehydrate_reach` to 16, so every retained archive is reachable for
  recall and restore.

## Tool surface

- `ctx_knowledge` (MCP): `rust/src/tools/ctx_knowledge/`,
  registered in `rust/src/tools/registered/ctx_knowledge.rs`
  - `action=policy` with `value=show` (the default): prints the effective
    knowledge and lifecycle values. Any other `value` returns an error.
  - `action=consolidate` (with `dry_run=true` ⇒ preview without writes)
  - `action=restore` (`store` = `facts|history|patterns`, `query`,
    `limit`, default 50) ⇒ lossless undo from the archive
- From a shell: `lean-ctx call ctx_knowledge --project-root <path> --json
  '{"action":"consolidate"}'` (or `restore`, with the same arguments)
- Relations: `rust/src/tools/ctx_knowledge_relations.rs` (bounded,
  deterministic outputs)

## Security & privacy

- No secrets in knowledge, artifacts or logs; redaction and path boundaries
  apply to memory I/O as well.
- Outputs are bounded so policies do not cause token burn.
