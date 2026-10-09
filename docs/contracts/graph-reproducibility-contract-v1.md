# Graph Reproducibility Contract v1

## Goal

`sobre_graph` answers are **deterministic** (stable ordering), **bounded**
(capped lists) and built from a graph that is **current** when the answer is
given.

This contract covers:

- Property graph storage under `<data_dir>/graphs/<project_hash>/graph.db`
- Graph build + freshness semantics (`sobre_graph action=build|status`)
- Deterministic outputs from `sobre_graph` (incl. `format=json`)

## Version

- `schema_version=1`: `GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION` in
  `rust/src/core/contracts.rs`

## Build triggers & freshness

### Automatic build

- Every `sobre_graph` action except `build` and `status` opens the graph
  through `open_or_build` (`rust/src/core/graph_provider.rs`): it builds the
  graph when there is none, and rebuilds it first when it is stale (a file's
  content changed since the build, or the build is older than 48 hours).
- A build that takes longer than 30 s goes on in the background; the action
  then says the graph is not ready yet.

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

- `sobre_graph action=status` reports whether the graph looks **fresh** or
  **stale**, or **unknown** when the build recorded no metadata.
- Staleness is determined by comparing build metadata (git head/dirty) to current repo state when available.

## Determinism (MUST)

Same repo snapshot ⇒ same **logical outputs** (stable ordering + stable truncation).

Rules:

- File enumeration during `build` is **sorted** lexicographically (relative paths).
- Resolved import targets are **sorted + deduplicated** before inserting edges.
- Traversal adjacency lists are **sorted + deduplicated** (BFS/DFS becomes deterministic).
- The `impact` and `changes` file lists are **stable-sorted**, then
  **truncated** with an explicit `truncated` flag.

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

The `impact` and `changes` affected-file lists are capped by
`IMPACT_AFFECTED_FILES_LIMIT` (200, `rust/src/core/budgets.rs`). Other
`sobre_graph` outputs have no cap.

## Storage

Graph DB + meta are written **only** under `<data_dir>/graphs/<project_hash>/`.
