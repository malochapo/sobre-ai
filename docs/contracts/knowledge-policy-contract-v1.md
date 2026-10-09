# Knowledge Policy Contract v1

## Purpose

A versioned governance contract for project knowledge (facts and the
history of consolidations) that is:

- **bounded**: stores and outputs have fixed caps,
- **deterministic**: stable ordering and stable semantics,
- **replayable**: a replaced value stays reachable (`recall as_of`),
- **redacted**: a value is scanned for secrets before it is stored.

## Version

- v1 (this document). The policy data carries no `schema_version` of its own.

## Policy source

The policy is `MemoryPolicy` (`rust/src/core/memory_policy.rs`). Every caller
uses `MemoryPolicy::default()`: there is no config-file section and no
environment override for it, so the values below are fixed.

### Knowledge (`KnowledgePolicy`)

| Field | Value |
|---|---|
| `max_facts` | 200 |
| `max_history` | 100 |
| `contradiction_threshold` | 0.5 |
| `recall_facts_limit` | 10 (`core/budgets.rs`) |

### Lifecycle (`LifecyclePolicy`)

| Field | Value |
|---|---|
| `low_confidence_threshold` | 0.3 |
| `stale_days` | 30 |
| `similarity_threshold` | 0.85 |
| `base_stability_days` | 90.0 |
| `prune_unretrieved_after_days` | 90 |
| `reclaim_headroom_pct` | 0.25 (a full store settles at 75%) |

`MemoryPolicy` also holds embeddings and admission sub-policies;
their defaults are in the same file. None of it is configurable.

## Semantics (MUST)

### Facts

- A fact is addressed by `(category, key)`.
- Current vs superseded:
  - current: `valid_until == None`
  - superseded: `valid_until != None`
- An update keeps the previous state: while a superseded version is still in
  the store, `recall` with `as_of` returns the version valid at that date.

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

Results are sorted with stable tie-breaks, then truncated.

### Lifecycle

Confidence decay (an Ebbinghaus curve), consolidation and compaction are
deterministic and bounded (`rust/src/core/memory_lifecycle.rs`), parameterised
by `base_stability_days`, `low_confidence_threshold`, `stale_days`,
`similarity_threshold` and `max_facts`.

### Eviction and archives

- **Capacity reclaim.** A store (facts, history) over its cap goes through
  the capacity manager (`rust/src/core/memory_capacity.rs::reclaim_store`),
  which archives the evicted tail **before** removing it: when the archive
  cannot be written, nothing is removed.
- **Lifecycle compaction** (`rust/src/core/memory_lifecycle.rs::compact`)
  removes facts of low confidence, stale facts, and facts never retrieved for
  `prune_unretrieved_after_days`, then archives them; a failed archive write
  is ignored.
- **Consolidation** merges near-duplicate current facts of one category
  (word similarity at least `similarity_threshold`): the lower-confidence one
  is dropped without an archive, its confirmations added to the one kept.
- Facts are archived at `memory/archive/archive-*.json`, shared by every
  project; history under
  `memory/archive/<store>/<project-hash>/archive-*.json`
  (`rust/src/core/memory_archive.rs`).
- **One formula plus hysteresis.** `reclaim_target(max, reclaim_headroom_pct)`
  (`max - ceil(max * pct)`, with `pct` clamped to `[0.0, 0.95]`) is the only
  target-size calculation. A reclaim triggers only when `len >= max` and then
  settles the store at `reclaim_target`, so there is no churn on every write.
- **Retention ranking** per store is deterministic (stable sort tie-breaks):
  facts by `is_current` then output ranking, history newest first.
- **Archive reachability.** `ArchiveConfig::default()` sets both `max_files`
  and `rehydrate_reach` to 16, so every retained facts archive is reachable:
  a `recall` that matches no current fact brings up to 3 matching archived
  facts back into the current project, whichever project archived them.

## Tool surface

- `sobre_knowledge` (MCP): `rust/src/tools/sobre_knowledge/`,
  registered in `rust/src/tools/registered/sobre_knowledge.rs`. Its actions
  are `remember`, `recall`, `search`, `status`, `remove`, `consolidate`
  (with `dry_run=true` ⇒ preview without writes) and `gotcha`.
- From a shell: `sobre call sobre_knowledge --project-root <path> --json
  '{"action":"consolidate"}'`

## Security & privacy

- `remember` scans a value for secrets and redacts them before storing it
  (`rust/src/tools/sobre_knowledge/remember.rs`).
- Recall outputs are capped by `recall_facts_limit`.
