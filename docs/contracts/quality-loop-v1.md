# Quality Loop v1 — Edit-Outcome Feedback into Mode Selection

Status: experimental (GL #494)
Owner: core engine
Consumers: `auto_mode_resolver`, `ctx_patch`

## Problem

`BounceTracker` closes the feedback loop for *re-read* bounces (compressed
read → full re-read). It does not cover edits that fail because the file was
last read in a compressed mode: the agent quotes an `old_string` from a
`map`/`signatures` rendering whose body was never in context, or edits against
line anchors that have since drifted. Without a separate signal, the next read
of a similar file would be compressed the same way.

## Signals

Edit outcomes are recorded by the two edit engines behind `ctx_patch`, keyed
by the **mode of the last lean-ctx read of that file** (session cache
`last_mode`):

- string replacement (`op=replace_unique`): `tools/impl_edit.rs`
  `record_outcome` → `core::edit_quality::record_edit_outcome`
- anchored ops (`set_line`, `replace_lines`, `insert_after`, `delete`, …):
  `tools/ctx_patch/mod.rs` `record_outcome` →
  `core::edit_quality::record_anchored_edit_outcome`

Only these outcomes carry signal:

| Outcome | Condition | Recorded as |
|---|---|---|
| Success | edit applied (`CacheEffect::Invalidate`) | success for `(ext, last_mode)` |
| Compression-correlated failure | string replacement: `old_string` not found (auto-escalation `CacheEffect::StoreFull`, or plain miss after a `full` read as baseline) | failure for `(ext, last_mode)` |
| Stale-anchor failure | anchored op returns `CONFLICT:` (the anchors the model edited against had drifted) | failure for `(ext, last_mode)` |

Explicitly **not** recorded (no compression signal): file creation
(`ctx_patch op=create`; a new file has no recorded read mode), empty or identical `old_string`/`new_string`,
preimage/TOCTOU mismatches, missing files, already-applied edits, structural
errors in anchored ops, and files never read through lean-ctx (`last_mode`
empty).

## Feedback rules

### 1. Per-path one-shot escalation

A failure arms a pending escalation for that path, unless the last read mode
already equals the escalation target. There are two independent one-shot
escalations (`core/edit_quality.rs`), checked first in
`core/auto_mode_resolver.rs::resolve`:

| Failure | Arms (when last mode ≠ target) | Next `mode=auto` resolution | Resolver source |
|---|---|---|---|
| Stale `ctx_patch` anchor (`CONFLICT:`) | anchored escalation | `anchored` (fresh line anchors, so the agent retries by reference) | `anchored_edit_fail_escalation` |
| `old_string` not found | full escalation | `full` | `edit_fail_escalation` |

The anchored escalation is checked before the full one. Each escalation is
consumed by the resolution that uses it, and pending escalations expire after
**1 hour**.

For string replacement this complements the immediate in-response escalation
that the edit engine appends (full content in the error message): the
in-response copy serves the retry, the pending escalation serves the next
independent read.

### 2. Per-(extension × mode) risky penalty

Aggregated per `(file extension, read mode)` pair with hysteresis:

```
fail_rate = fails / (fails + successes)

enter risky:  fails >= 2  AND  fail_rate >= 0.25
exit  risky:  fail_rate < 0.15
```

While a pair is risky, `mode=auto` resolutions that would pick that mode for
that file type return `full` instead (resolver source:
`edit_quality_penalty`). The two thresholds prevent flapping: a single lucky
edit cannot immediately re-enable a mode that keeps breaking edits.

The penalty is **per extension**, never global: `rs|map` being risky does not
affect `py|map` or `rs|signatures`.

## Persistence

`edit_quality.json` in the data directory (respects `LEAN_CTX_DATA_DIR`), atomic
tmp+rename writes, flushed every 10 recordings and on server shutdown.

Bounds:

| Limit | Value |
|---|---|
| Pair decay (no failure) | 30 days |
| Escalation TTL | 1 hour |
| Max pairs | 200 (oldest-failure evicted) |
| Max pending escalations | 100 per escalation kind (oldest evicted) |

## Observability

The store has no report surface; its effect shows in auto-mode resolution,
where the sources `anchored_edit_fail_escalation`, `edit_fail_escalation` and
`edit_quality_penalty` appear.

## Invariants

- Recording an outcome never fails an edit: store access goes through a
  process-global mutex, and a poisoned lock is skipped silently.
- The risky penalty (rule 2) only ever replaces the resolved mode with `full`;
  it never picks a lossier mode than the resolver would have chosen. The
  one-shot escalations (rule 1) return `full` or `anchored`, depending on
  which kind of failure armed them.
- No regression of bounce semantics: `BounceTracker` remains an independent
  signal, evaluated before the quality loop's penalty.
