# Conformance & Reproducibility — `conformance-v1`

Status: stable · EPIC 12.17 · Code: [`rust/src/core/conformance.rs`](../../rust/src/core/conformance.rs)

A self-check any user or CI can run to prove that the lossy read modes keep
what an agent needs and that the registered compressors, chunkers and read
modes behave.

## Running it

```bash
lean-ctx conformance          # human-readable scorecard
lean-ctx conformance --json   # machine-readable (CI / dashboards)
```

Exit code is non-zero if any check fails, so it gates cleanly in CI. The same
suite runs as `rust/tests/suite/conformance_suite.rs`.

## What it checks

| Category | Check | Invariant |
|----------|-------|-----------|
| `accuracy` | `read_mode_deterministic:<mode>` | A lossy read mode renders the fixture identically twice. |
| `accuracy` | `read_mode_keeps_symbols:<mode>` | The mode keeps the fixture's public symbols. |
| `accuracy` | `read_mode_strips_bodies:<mode>` | The mode drops function bodies. |
| `accuracy` | `read_mode_compresses:<mode>` | The mode output is smaller than the source. |
| `accuracy` | `density_respects_budget:0.4` / `density_deterministic:0.4` | `density:0.4` stays within its budget and is deterministic. |
| `extensions` | `compressor:<name>` | Deterministic for equal input; output never exceeds a byte budget; never splits a UTF-8 char. |
| `extensions` | `chunker:<name>` | Deterministic; empty input ⇒ no chunks; non-empty input ⇒ ≥1 non-empty chunk. |
| `extensions` | `read_mode:<name>` | Deterministic; `full` round-trips source verbatim. |

The extension checks run against **every registered** compressor / chunker /
read-mode, so a transform that is non-deterministic or exceeds its budget fails
conformance immediately.

## Scorecard shape (`--json`)

```json
{
  "version": 1,
  "passed": 2,
  "total": 2,
  "all_passed": true,
  "checks": [
    { "name": "read_mode_deterministic:map", "category": "accuracy", "passed": true, "detail": "" },
    { "name": "compressor:identity", "category": "extensions", "passed": true, "detail": "" }
  ]
}
```

## Versioning

`conformance-v1` is additive: new checks/categories may be added in a minor
revision. Removing a check or weakening an invariant is a breaking change
requiring `-v2`. The corpus the extension invariants run against is an
implementation detail and may grow.
