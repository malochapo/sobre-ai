# Quality Loop v1 — Edit-Outcome Feedback into Mode Selection

Status: experimental (GL #494)
Owner: core engine
Consumers: `auto_mode_resolver`, `ctx_patch`

## Problem

`BounceTracker` and `path_mode_memory` close the feedback loop for *re-read*
bounces (compressed read → full re-read), but an edit that fails because the
file was last read in a compressed mode taught the system nothing: the agent
quoted an `old_string` from a `map`/`signatures` rendering whose body was
never in context, the edit missed, and the next read of a similar file was
compressed exactly the same way.

## Signals

Edit outcomes are recorded by the edit engine behind `ctx_patch`
(`tools/impl_edit.rs`, via `core::edit_quality::record_edit_outcome`; anchored
edits via `record_anchored_edit_outcome`),
keyed by the **mode of the last lean-ctx read of that file** (session cache
`last_mode`). Only two outcomes carry signal:

| Outcome | Condition | Recorded as |
|---|---|---|
| Success | replacement applied (`CacheEffect::Invalidate`) | success for `(ext, last_mode)` |
| Compression-correlated failure | `old_string` not found (auto-escalation `CacheEffect::StoreFull`, or plain miss after a `full` read as baseline) | failure for `(ext, last_mode)` |

Explicitly **not** recorded (no compression signal): `create=true`, empty or
identical `old_string`/`new_string`, preimage/TOCTOU mismatches, missing
files, already-applied edits, and files never read through lean-ctx
(`last_mode` empty).

## Feedback rules

### 1. Per-path one-shot escalation

A compression-correlated failure (last mode ≠ `full`) arms a pending
escalation for that path. The **next** `mode=auto` resolution of the same
path returns `full` (resolver source: `edit_fail_escalation`), then the
escalation is consumed. Pending escalations expire after **1 hour**.

This complements the immediate in-response escalation that the edit engine already
appends (full content in the error message): the in-response copy serves the
retry, the pending escalation serves the next independent read.

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
| Max pending escalations | 100 (oldest evicted) |

## Observability

The store has no report surface; its effect shows in auto-mode resolution,
where the sources `edit_fail_escalation` and `edit_quality_penalty` appear.

## Invariants

- Recording an outcome never blocks an edit: store access is lock-guarded and
  failures to lock are silently skipped.
- The penalty only ever *escalates* toward `full`; it never picks a lossier
  mode than the resolver would have chosen.
- No regression of bounce semantics: `BounceTracker` / `path_mode_memory`
  remain independent signals evaluated before the quality loop's penalty.
