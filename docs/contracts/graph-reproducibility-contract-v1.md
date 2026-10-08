# Graph Reproducibility Contract v1

GitLab: `#2318`

## Goal

Make graph-driven tooling **reproducible** (CI/proofs), **deterministic** (stable ordering), **bounded** (output caps), and **safe-by-default**.

This contract covers:

- Property graph storage under `$SOBRE_DATA_DIR/graphs/<project_hash>/graph.db`
- Graph build + freshness semantics (`ctx_impact action=build|status`)
- Deterministic outputs from `ctx_impact` and `ctx_graph` (incl. `format=json`)

## Version

- `schema_version=1`: `GRAPH_REPRODUCIBILITY_V1_SCHEMA_VERSION` in
  `rust/src/core/contracts.rs`

## Build triggers & freshness

### Auto-build (zero-config)

- Tools **may** auto-build the property graph when it is missing/empty:
  - `ctx_impact action=analyze` (auto-build if empty)

### Explicit rebuild

- `ctx_impact action=build` rescans the project and rewrites `graph.db` (in
  `$SOBRE_DATA_DIR/graphs/<project_hash>/`) and `graph.meta.json` alongside
  it. Files whose content hash is unchanged are reused, so the result equals a
  build from scratch.
- There is one extractor: `ctx_impact build`, `ctx_graph build` and
  `sobre index build` all run the same scan.

### Edge semantics

- Dependencies, dependents, impact and chains follow code references only:
  imports, module declarations and re-exports, and type uses.
- `related` also follows two heuristics: files changed together in git and
  files in the same directory.

### Freshness check

- `ctx_impact action=status` reports whether the graph looks **fresh** or **stale**.
- Staleness is determined by comparing build metadata (git head/dirty) to current repo state when available.

## Determinism (MUST)

Same repo snapshot + same policies ⇒ same **logical outputs** (stable ordering + stable truncation).

Rules:

- File enumeration during `build` is **sorted** lexicographically (relative paths).
- Resolved import targets are **sorted + deduplicated** before inserting edges.
- Traversal adjacency lists are **sorted + deduplicated** (BFS/DFS becomes deterministic).
- Output lists are **stable-sorted**, then **truncated** with explicit `truncated` markers.

## Output format (tools)

`ctx_impact` and `ctx_graph action=neighbors|path` accept:

- `format=text|json` (default: `text`)

When `format=json`:

- Output is machine-readable JSON (no token-suffix lines).
- Payload includes:
  - `schema_version` (Graph Reproducibility Contract v1)
  - `tool`, `action`
  - `project.project_root_hash` + `project.project_identity_hash` (hash-only; never leak identity strings)
  - `graph` summary (exists, nodes, edges, db_path)
  - action-specific fields + `truncated` flags

## Boundedness (MUST)

`ctx_impact` affected-file lists are capped by `IMPACT_AFFECTED_FILES_LIMIT`
(`rust/src/core/budgets.rs`) to prevent DoS/token-burn. `ctx_graph` outputs
are not budgeted.

## Security & privacy

- Graph DB + meta are written **only** under `$SOBRE_DATA_DIR/graphs/<project_hash>/`.
- No secrets must appear in graph artifacts, logs, or exports.

