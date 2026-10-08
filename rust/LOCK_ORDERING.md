# Lock ordering

This document lists every global/static lock and the notable `Arc<Mutex/RwLock>` fields in the
codebase, defines the acquisition order, and records the rules for async code.

Locations are given as file paths relative to `rust/src/`, without line numbers.
`tests/suite/lock_ordering_check.rs` fails CI when a production `static` `Mutex`/`RwLock` under
`src/` has no row in a table below.

---

## 1. Global / Static Locks

All `std::sync` unless noted otherwise. "fn-local" means the static is declared inside an
accessor function.

| # | Lock | File | Type | Purpose |
|---|------|------|------|---------|
| L1 | `REGISTRY` | `core/index_orchestrator.rs` | `OnceLock<Mutex<HashMap<String, Arc<Mutex<ProjectBuild>>>>>` | Outer map of per-project build state |
| L2 | per-project `ProjectBuild` | `core/index_orchestrator.rs` (inner) | `Arc<Mutex<ProjectBuild>>` | Individual project build progress |
| L3 | `HEATMAP_BUFFER` | `core/heatmap.rs` | `Mutex<Option<HeatMap>>` | Buffered access-frequency heatmap |
| L4 | `Config::CACHE` | `core/config/loader.rs` (fn-local in `Config::load_arc`) | `Mutex<Option<(Arc<Config>, Option<String>)>>` | Shared config plus the content hash of the file it was built from; rebuilt only when the hash changes |
| L7 | `STATS_BUFFER` | `core/stats/mod.rs` | `Mutex<Option<(StatsStore, StatsStore, Instant)>>` | Token-savings statistics |
| L8 | `COST_BUFFER` | `core/a2a/cost_attribution.rs` | `Mutex<Option<CostStore>>` | A2A cost tracking |
| L17 | `FILE_LOCKS` | `core/path_locks.rs` (fn-local in `per_file_lock`) | `OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>>` | Per-file serialization of reads and edits for concurrent subagents |
| L20 | `RECENT` | `core/auto_findings.rs` | `Mutex<Vec<RecentEntry>>` | Recent auto-finding entries |
| L22 | `BACKENDS` | `lsp/router.rs` | `LazyLock<Mutex<HashMap<String, Box<dyn LspBackend>>>>` | Per-language language-server client registry; on a miss the client is started from the built-in server table while the lock is held. Independent leaf lock |
| L27 | `CACHE` (git) | `core/git_cache.rs` | `LazyLock<Mutex<GitCache>>` | Cached git metadata (branch, status) |
| L33 | `CACHE` (search) | `core/search_index.rs` | `OnceLock<Mutex<HashMap<String, CacheEntry>>>` | Search index query cache |
| L34 | `GLOBAL` (capabilities) | `core/client_capabilities.rs` | `OnceLock<Mutex<ClientMcpCapabilities>>` | Client MCP capability flags |
| L37 | `LAST_REAL` | `proxy/introspect.rs` | `Mutex<[Option<String>; 3]>` | Last 3 real (non-proxy) request paths |
| L38 | `GLOBAL_TRACKER` | `core/bounce_tracker.rs` | `OnceLock<Mutex<BounceTracker>>` | Tracks repeated tool-call bounces |
| L41 | `KNOWLEDGE_LOCKS` | `core/knowledge/persist.rs` (fn-local) | `OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>>` | Per-project knowledge.json read-modify-write serialization (same outer/inner pattern as L1/L2) |
| L42 | `LAST_PARSE_ERROR` | `core/config/loader.rs` | `Mutex<Option<String>>` | Most recent global `config.toml` parse error (surfaced by doctor/diagnostics); never held together with L4 |
| L47 | `SOURCE_COUNTS` | `core/auto_mode_resolver.rs` | `Mutex<Option<HashMap<&'static str, u64>>>` | Per-process counters of which signal decided each auto-mode resolution; surfaced by `ctx_metrics` |
| L48 | `NO_GIT_ROOTS` | `core/git_signals.rs` | `Mutex<Option<HashSet<String>>>` | Roots probed and found non-git — negative cache so each root is probed at most once per process |
| L50 | `LINE_EMBED_CACHE` | `core/entropy.rs` (fn-local) | `Mutex<Option<HashMap<u64, Vec<f32>>>>` | Per-line embedding cache (line-hash → vector) for the semantic redundancy filter; capacity-bounded, never blocks on model loads |
| L53 | `REGISTRY` (introspect) | `core/introspect.rs` | `LazyLock<Mutex<Registry>>` | Activity registry — per-subsystem tick counters + last-run; flushed debounced to a project-scoped JSON so `introspect cognition` / `doctor` can report wired/active across processes |
| L56 | `MEMO` | `proxy/prose_ranker.rs` | `Mutex<Option<HashMap<u64, String>>>` | Wire-prose squeeze memo: the first squeeze of a `(content, budget)` is frozen for the process lifetime so a later recompute returns identical bytes (provider prompt-cache stability); capacity-bounded, independent leaf lock |
| L57 | `SEEN` | `core/conversation.rs` (fn-local) | `OnceLock<RwLock<Vec<(String, Instant)>>>` | Recent sightings of distinct conversation ids feeding the multi-conversation detector; capacity-pruned, independent leaf lock |
| L58 | `STORE` (conversation) | `core/conversation.rs` (fn-local) | `OnceLock<RwLock<Option<Cached>>>` | Cached current conversation id with its refresh instant; independent leaf lock, never held together with L57 |
| L62 | `TARGETS` | `core/eviction_orchestrator.rs` | `LazyLock<Mutex<EvictionRegistry>>` | Registry of live `EvictionOrchestrator` weak refs: `register()`/`on_memory_pressure()` lock it only to clone/upgrade the target list, then drop it before calling `on_pressure()` on each target; never held while any other lock is acquired |
| L63 | `BLOCK` | `core/events.rs` (fn-local in `next_event_id`) | `OnceLock<Mutex<LocalBlock>>` | Per-process cache of the current pre-reserved event-id block: most calls bump `next` under the lock; on exhaustion the lock is held across `reserve_event_id_block_at`, which takes its own OS file lock (`events.seq.lock`, not a Rust static) — never nested with another static Rust lock |
| L65 | `SESSIONS` | `proxy/sticky_tools.rs` (fn-local) | `OnceLock<Mutex<HashSet<u64>>>` | Active proxy conversation tracking; capacity-bounded with oldest-first eviction; independent leaf lock |
| L66 | `JOBS` | `server/background_shell.rs` | `LazyLock<Mutex<HashMap<String, Job>>>` | Content-addressed background shell jobs; held only to insert, inspect, or remove a job, released before executing or joining its worker thread; independent leaf lock |
| L67 | `BUILD_GATE` | `core/graph_provider.rs` | `LazyLock<Mutex<()>>` | Serializes synchronous graph-index builds; acquired with `try_lock()` and held only while the spawned builder is awaited |
| L87 | `CACHE` (ANN) | `core/ann_cache.rs` (fn-local) | `OnceLock<Mutex<Option<Cached>>>` | Cached approximate-nearest-neighbour index keyed by a vector fingerprint; independent leaf lock |
| L88 | `CACHE` (content) | `core/content_cache.rs` | `OnceLock<Mutex<Cache>>` | Bounded file-content cache shared by the search-index build and `ctx_search`, validated by `(mtime, size)`; independent leaf lock |
| L89 | `STORE` (diagnostics) | `core/diagnostics_store.rs` | `OnceLock<Mutex<DiagnosticsStore>>` | Active compiler/linter diagnostics (TTL- and size-bounded) used as a context-priority signal; independent leaf lock |
| L90 | `STORE` (edit quality) | `core/edit_quality.rs` | `OnceLock<Mutex<EditQualityStore>>` | Per-(extension × read mode) edit success/failure stats for the quality loop; independent leaf lock |
| L92 | `ACTIVE` | `core/gotcha_tracker/runtime.rs` | `OnceLock<Mutex<Option<Active>>>` | Active per-project gotcha store, kept in memory so pending errors survive across shell calls; independent leaf lock |
| L93 | `MAP` | `core/index_progress.rs` (fn-local) | `OnceLock<Mutex<ProgressMap>>` | Index build progress snapshots per `(root, component)`; independent leaf lock |
| L94 | `G` | `core/read_stub_index.rs` (fn-local) | `OnceLock<RwLock<ReadStubIndex>>` | Persistent, conversation-scoped index of fully delivered file reads (bookkeeping only, never content) used to serve the `[unchanged]` stub across restarts; independent leaf lock |
| L95 | `TRACKER` (relevance) | `core/relevance_tracker.rs` | `LazyLock<Mutex<RelevanceTracker>>` | Bounded relevance-tracking entries (keywords, age, token budget); independent leaf lock |
| L96 | `STORE` (cache attribution) | `proxy/cache_attribution.rs` (fn-local) | `OnceLock<Mutex<HashMap<u64, PrefixState>>>` | Per-prefix provider cache state for cold-start / warm-reuse / TTL-lapse attribution; independent leaf lock |
| L97 | `STORE` (proxy cost) | `proxy/cost.rs` (fn-local) | `OnceLock<Mutex<HashMap<String, ModelAccum>>>` | Per-model cost accumulators, capped at 256 model buckets; independent leaf lock |
| L98 | `STORE` (prefix replay) | `proxy/prefix_replay.rs` (fn-local) | `OnceLock<Mutex<HashMap<u64, ConversationPrefix>>>` | Forwarded prefix bytes per conversation, replayed so an unchanged prefix is re-sent byte-identical; independent leaf lock |
| L99 | `STORE` (usage meter) | `proxy/usage_meter.rs` (fn-local) | `OnceLock<Mutex<HashMap<String, ModelUsage>>>` | Per-model proxy usage counters, capped at 256 model buckets; independent leaf lock |

### Test / Environment Locks (serialise env-var and cwd mutations)

| # | Lock | File | Type | Purpose |
|---|------|------|------|---------|
| E4 | `LOCK` (data dir) | `core/data_dir.rs` (fn-local in `test_env_lock`) | `OnceLock<Mutex<()>>` | Serializes tests that mutate data-dir environment variables |
| E6 | `LOCK` (knowledge cwd) | `tools/ctx_knowledge/tests.rs` (fn-local in `CurrentDirGuard::enter`) | `OnceLock<Mutex<()>>` | Serializes `ctx_knowledge` tests that change the process working directory |

Locks declared inside `#[cfg(test)]` items are test-only and are not listed here.

---

## 2. Arc-wrapped Session Locks (per-MCP-session, `tokio::sync::RwLock`)

Fields of `LeanCtxServer` in `tools/server.rs`:

| Field | Type | Purpose |
|-------|------|---------|
| `cache` | `Arc<RwLock<SessionCache>>` | File content cache |
| `session` | `Arc<RwLock<SessionState>>` | Session metadata |
| `tool_calls` | `Arc<RwLock<Vec<ToolCallRecord>>>` | Call log |
| `last_call` | `Arc<RwLock<Instant>>` | Idle-timeout tracking |
| `agent_id` | `Arc<RwLock<Option<String>>>` | Current agent identifier |
| `presence_agent_id` | `Arc<RwLock<Option<String>>>` | Agent id registered for this MCP process; used for presence heartbeats |
| `client_name` | `Arc<RwLock<String>>` | Connected client name |
| `loop_detector` | `Arc<RwLock<LoopDetector>>` | Loop-detection state |
| `ledger` | `Arc<RwLock<ContextLedger>>` | Context ledger |
| `context_ir` | `Option<Arc<RwLock<ContextIrV1>>>` | Context IR state |
| `peer` | `Arc<RwLock<Option<Peer<RoleServer>>>>` | MCP client peer handle |
| `bm25_cache` | `Arc<std::sync::Mutex<Option<Bm25CacheEntry>>>` | Session BM25 index cache (`core/bm25_cache.rs`, `SharedBm25Cache`); a **`std::sync::Mutex`**, not tokio — never held across `.await` |

Apart from `bm25_cache`, these are **`tokio::sync::RwLock`** and are scoped to a single
session — no cross-session nesting is expected. Tool handlers receive `cache`, `session` and
`tool_calls` through `ToolContext` (`server/tool_trait.rs`).

### Other Arc-wrapped Locks

| Lock | File | Type | Purpose |
|------|------|------|---------|
| `SharedProtocol` | `mcp_stdio.rs` | `Arc<Mutex<Option<WireProtocol>>>` | Detected MCP stdio wire protocol (std::sync) |
| stdio transport `write` | `mcp_stdio.rs` | `Arc<tokio::sync::Mutex<Option<TransportWriter>>>` | MCP stdio framed writer; held across the async send of one message |

---

## 3. Lock Acquisition Order

### Rule: always acquire outer → inner.

```
L1 (REGISTRY outer map)
 └─► L2 (per-project ProjectBuild)     — NEVER hold L1 while locking L2
```

`entry_for()` in `core/index_orchestrator.rs` enforces this: it locks L1, clones the
`Arc<Mutex<ProjectBuild>>`, **drops** L1, then the caller locks L2 independently. L1 and L2 are
never held simultaneously. L41 (`KNOWLEDGE_LOCKS`) follows the same pattern.

### Per-file Path Lock (L17)

L17 lives in the shared `core::path_locks` registry (`per_file_lock`) and is used by **both**
`ctx_read` (`tools/registered/ctx_read.rs`) and the edit path, `ctx_patch`
(`tools/registered/ctx_patch.rs`, `tools/ctx_patch/`; its `replace_unique` op runs through the
string-replacement engine in `tools/registered/impl_edit.rs` and `tools/impl_edit.rs`). The outer `Mutex<HashMap>` is held
briefly to clone the per-path `Arc<Mutex<()>>`, then dropped before the per-file lock is
acquired. The per-file lock is acquired before the session cache lock. This serializes
concurrent operations on the *same* path; operations on different files proceed independently.

**Edit path:** `ctx_patch` acquires the L17 per-file lock (bounded `try_lock()` loop, 30s
deadline) and then performs **all** disk I/O — read preimage, apply, TOCTOU recheck, atomic
rename — *without* holding the session cache write-lock. The cache lock is taken only twice,
each briefly and with a 5s timeout: a shared `read()` to fetch the recorded read mode before the
I/O, and an exclusive `write()` to apply the deferred `CacheEffect` (invalidate / store-full)
after the I/O. Same-file edit correctness is guaranteed by the TOCTOU preimage guard plus the
atomic temp-file rename inside `run_io`, not by the cache lock. The `replace_all` operation
holds the per-file lock with a blocking `lock()` for the write, then invalidates the cache
entry under a brief write lock (2s timeout).

**Read path (spawned thread):** `ctx_read`'s slow path runs in a spawned thread:

1. **Phase 1 (shared lock):** Try the `[unchanged]` stub under `cache.try_read()`; on a hit,
   return without taking the write lock.
2. **Phase 2a (no lock):** Disk I/O (`read_file_lossy`) under the per-file lock only, *without*
   the cache lock.
3. **Phase 2b-i (brief write lock, 10s deadline):** prepare cache state, resolve the mode,
   check for hits.
4. **Phase 2b-ii (no lock):** heavy computation (parsing, compression, redaction).
5. **Phase 2b-iii (brief write lock, 5s deadline):** store the result and metadata; on timeout
   the result is returned without caching.

**Bounded waits:** lock acquisitions inside the spawned thread use `try_lock()`/`try_write()`
loops with deadlines, inside a 30s `recv_timeout` guard. When `recv_timeout` fires, a
cancellation flag is set so the thread exits promptly instead of holding locks.

```
thread::spawn {
    L17 outer (FILE_LOCKS map)        — held briefly to clone Arc, then dropped
     └─► L17 inner (per-file Mutex)   — try_lock() with deadline
          ├─► Phase 1:   cache.try_read()  — stub hit? → return early
          ├─► Phase 2a:  read_file_lossy() — disk I/O, NO cache lock
          ├─► Phase 2b-i:   cache.try_write() — brief prepare
          ├─► Phase 2b-ii:  compute            — NO cache lock
          └─► Phase 2b-iii: cache.try_write() — brief store
}
```

The inline fast path (no thread) keeps the Phase 1 read-lock stub check and otherwise calls
`handle_with_task_resolved_tuned` under the write lock.

### Worker Thread Tuning

The Tokio runtime worker thread count is `available_parallelism().clamp(1, 4)`. The blocking
thread pool is `worker_threads * 4`, clamped to `[8, 32]` (`cli/dispatch/server.rs`).

### Independent Static Locks

All static locks other than the L1 → L2 and L17 / L41 outer → inner pairs are **independent
singletons**: they protect isolated subsystem state and are never nested inside each other.
Each is acquired in isolation:

- **Do not hold two static locks at the same time.** If a future change requires locking two
  subsystems, add the ordering rule here first.
- **Hold locks for the minimum duration.** Clone/copy data out, drop the guard, then do work.

### Session Locks

Session-scoped locks on `LeanCtxServer` are logically independent:

- Acquire at most **one session lock per tool handler** at a time, except the sequences
  documented above (per-file lock → cache lock; cache lock released → session lock in the edit
  path).
- If you must hold two, acquire in field-declaration order (cache → session → tool_calls → …).
- **Never hold a session `RwLock` while locking a global static Mutex** — this risks priority
  inversion between the tokio runtime and OS threads.

### Test/Environment Locks (E-numbered)

These exist solely to serialise tests that mutate environment variables or the working
directory. They must not be held across any other lock acquisition.

---

## 4. Async Code: `tokio::sync::Mutex` vs `std::sync::Mutex`

| Use | When |
|-----|------|
| `std::sync::Mutex` | Lock held briefly (no `.await` while held), or lock is static/global |
| `tokio::sync::Mutex` | Lock must be held **across** `.await` points, or guards must be `Send` for spawned futures |
| `tokio::sync::RwLock` | Readers dominate, writers are rare; lock may be held across `.await` |

### Current usage

- **Global statics** → all `std::sync::Mutex` / `std::sync::RwLock` (held briefly, no await)
- **Session state** (`tools/server.rs`) → `tokio::sync::RwLock` (accessed from async tool
  handlers), except `bm25_cache`, which is a `std::sync::Mutex`
- **MCP stdio writer** (`mcp_stdio.rs`) → `tokio::sync::Mutex` (held across the async write of
  one framed message)

### Rules

1. **Never `.await` while holding a `std::sync::Mutex` guard.** The tokio runtime thread will
   block, starving other tasks.
2. **Prefer `std::sync::Mutex` for global caches** where the critical section is a quick
   read/write with no I/O.
3. **Use `tokio::sync::Mutex` only when the critical section contains `.await`.**
4. A `std::sync::MutexGuard` is `!Send` — it cannot be held across an `.await` in a spawned
   future. The compiler enforces this.

---

## 5. Adding New Locks — Checklist

1. Determine scope: global static vs per-session vs per-request.
2. Choose `std::sync` vs `tokio::sync` per Section 4.
3. Assign a lock number (append to Section 1) and document the acquisition order here.
4. If nesting is required, document the outer → inner relationship in Section 3.
5. Run `cargo check --all-targets` and `cargo check --all-targets --features dense` to verify
   `Send`/`Sync` bounds.
