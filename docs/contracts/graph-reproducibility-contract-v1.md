# Graph Reproducibility Contract v1

## Goal

Make graph-driven tooling **reproducible** (CI/proofs), **deterministic** (stable ordering), **bounded** (output caps), and **safe-by-default**.

This contract covers:

- Property graph storage under `<data_dir>/graphs/<project_hash>/graph.db`
- Graph build + freshness semantics (`sobre_graph action=build|status`)
- Deterministic outputs from `sobre_graph` (incl. `format=json`)

## Version

- `schema_version=1`: `GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION` in
  `rust/src/core/contracts.rs`

## Build triggers & freshness

### Auto-build (zero-config)

- Tools **may** auto-build the property graph when it is missing/empty:
  - `sobre_graph action=impact|changes` (auto-build if empty or built by an
    older engine)

### Explicit rebuild

- `sobre_graph action=build` rescans the project and rewrites `graph.db` (in
  `<data_dir>/graphs/<project_hash>/`) and `graph.meta.json` alongside
  it. Files whose content hash is unchanged are reused, so the result equals a
  build from scratch.
- There is one extractor: `sobre_graph build` and `sobre index build` run the
  same scan.

### Edge semantics

- Dependencies, dependents, impact and chains follow code references only:
  imports, module declarations and re-exports, and type uses.
- `related` also follows two heuristics: files changed together in git and
  files in the same directory.

### Freshness check

- `sobre_graph action=status` reports whether the graph looks **fresh** or **stale**.
- Staleness is determined by comparing build metadata (git head/dirty) to current repo state when available.

## Determinism (MUST)

Same repo snapshot + same policies ⇒ same **logical outputs** (stable ordering + stable truncation).

Rules:

- File enumeration during `build` is **sorted** lexicographically (relative paths).
- Resolved import targets are **sorted + deduplicated** before inserting edges.
- Traversal adjacency lists are **sorted + deduplicated** (BFS/DFS becomes deterministic).
- Output lists are **stable-sorted**, then **truncated** with explicit `truncated` markers.

## Output format (tools)

`sobre_graph action=neighbors|path|impact|changes` accept:

- `format=text|json` (default: `text`)

When `format=json`:

- Output is machine-readable JSON (no token-suffix lines).
- The `impact` payload includes:
  - `schema_version` (Graph Reproducibility Contract v1)
  - `tool`, `action`
  - `project.project_root_hash` (the hash of the canonical root; never the path)
  - `graph` summary (exists, nodes, edges, db_path)
  - action-specific fields + `truncated` flags

## Boundedness (MUST)

`sobre_graph action=impact` affected-file lists are capped by `IMPACT_AFFECTED_FILES_LIMIT`
(`rust/src/core/budgets.rs`) to prevent DoS/token-burn. `sobre_graph` outputs
are not budgeted.

## Security & privacy

- Graph DB + meta are written **only** under `<data_dir>/graphs/<project_hash>/`.
- No secrets must appear in graph artifacts, logs, or exports.

