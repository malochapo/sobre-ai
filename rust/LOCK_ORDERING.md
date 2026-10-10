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
| L4 | `Config::CACHE` | `core/config/loader.rs` (fn-local in `Config::load_arc`) | `Mutex<Option<(Arc<Config>, Option<String>)>>` | Shared config plus the content hash of the file it was built from; rebuilt only when the hash changes |
| L17 | `FILE_LOCKS` | `core/path_locks.rs` (fn-local in `per_file_lock`) | `OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>>` | Per-file serialization of reads and edits for concurrent subagents |
| L22 | `BACKENDS` | `lsp/router.rs` | `LazyLock<Mutex<HashMap<String, Box<dyn LspBackend>>>>` | Language-server clients, one per language and project root; on a miss the client is started from the built-in server table while the lock is held, and each request runs under it. Independent leaf lock |
| L27 | `CACHE` (git) | `core/git_cache.rs` | `LazyLock<Mutex<GitCache>>` | Cached git metadata (branch, status) |
| L33 | `CACHE` (search) | `core/search_index.rs` | `OnceLock<Mutex<HashMap<String, CacheEntry>>>` | Search index query cache |
| L38 | `GLOBAL_TRACKER` | `core/bounce_tracker.rs` | `OnceLock<Mutex<BounceTracker>>` | Tracks repeated tool-call bounces |
| L41 | `KNOWLEDGE_LOCKS` | `core/knowledge/persist.rs` (fn-local) | `OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>>` | Per-project knowledge.json read-modify-write serialization (same outer/inner pattern as L1/L2) |
| L42 | `LAST_PARSE_ERROR` | `core/config/loader.rs` | `Mutex<Option<String>>` | Most recent global `config.toml` parse error (surfaced by doctor/diagnostics); never held together with L4 |
| L48 | `NO_GIT_ROOTS` | `core/git_signals.rs` | `Mutex<Option<HashSet<String>>>` | Roots probed and found non-git — negative cache so each root is probed at most once per process |
| L50 | `LINE_EMBED_CACHE` | `core/entropy.rs` (fn-local) | `Mutex<Option<HashMap<u64, Vec<f32>>>>` | Per-line embedding cache (line-hash → vector) for the semantic redundancy filter; capacity-bounded, never blocks on model loads |
| L57 | `SEEN` | `core/conversation.rs` (fn-local) | `OnceLock<RwLock<Vec<(String, Instant)>>>` | Recent sightings of distinct conversation ids feeding the multi-conversation detector; capacity-pruned, independent leaf lock |
| L58 | `STORE` (conversation) | `core/conversation.rs` (fn-local) | `OnceLock<RwLock<Option<Cached>>>` | Cached current conversation id with its refresh instant; independent leaf lock, never held together with L57 |
| L62 | `TARGETS` | `core/eviction_orchestrator.rs` | `LazyLock<Mutex<EvictionRegistry>>` | Registry of live `EvictionOrchestrator` weak refs: `register()`/`on_memory_pressure()` lock it only to clone/upgrade the target list, then drop it before calling `on_pressure()` on each target; never held while any other lock is acquired |
| L66 | `JOBS` | `server/background_shell.rs` | `LazyLock<Mutex<HashMap<String, Job>>>` | Content-addressed background shell jobs; held only to insert, inspect, or remove a job, released before executing or joining its worker thread; independent leaf lock |
| L67 | `BUILD_GATE` | `core/graph_provider.rs` | `LazyLock<Mutex<()>>` | Serializes synchronous graph-index builds; acquired with a `try_lock()` loop (30s deadline) and held only while the spawned builder is awaited |
| L87 | `CACHE` (ANN) | `core/ann_cache.rs` (fn-local) | `OnceLock<Mutex<Option<Cached>>>` | Cached approximate-nearest-neighbour index keyed by a vector fingerprint; independent leaf lock |
| L88 | `CACHE` (content) | `core/content_cache.rs` | `OnceLock<Mutex<Cache>>` | Bounded file-content cache shared by the search-index build and `sobre_search`, validated by `(mtime, size)`; independent leaf lock |
| L89 | `STORE` (diagnostics) | `core/diagnostics_store.rs` | `OnceLock<Mutex<DiagnosticsStore>>` | Active compiler/linter diagnostics (TTL- and size-bounded) used as a context-priority signal; independent leaf lock |
| L90 | `STORE` (edit quality) | `core/edit_quality.rs` | `OnceLock<Mutex<EditQualityStore>>` | Per-(extension × read mode) edit success/failure stats for the quality loop; independent leaf lock |
| L93 | `MAP` | `core/index_progress.rs` (fn-local) | `OnceLock<Mutex<ProgressMap>>` | Index build progress snapshots per `(root, component)`; independent leaf lock |
| L94 | `G` | `core/read_stub_index.rs` (fn-local) | `OnceLock<RwLock<ReadStubIndex>>` | Persistent, conversation-scoped index of fully delivered file reads (bookkeeping only, never content) used to serve the `[unchanged]` stub across restarts; independent leaf lock |

### Test / Environment Locks (serialise env-var and cwd mutations)

| # | Lock | File | Type | Purpose |
|---|------|------|------|---------|
| E4 | `LOCK` (data dir) | `core/data_dir.rs` (fn-local in `test_env_lock`) | `OnceLock<Mutex<()>>` | Serializes tests that mutate data-dir environment variables |
| E6 | `LOCK` (knowledge cwd) | `tools/sobre_knowledge/tests.rs` (fn-local in `CurrentDirGuard::enter`) | `OnceLock<Mutex<()>>` | Serializes `sobre_knowledge` tests that change the process working directory |

Locks declared inside `#[cfg(test)]` items are test-only and are not listed here.

### Cross-process File Locks

OS advisory locks (`flock` on Unix, `LockFileEx` on Windows) taken through std's
`File::lock` / `File::try_lock`. The OS releases one when its handle closes or its process
dies, so none outlives its holder. Files are in the data directory unless noted; the
`.<name>.lock` ones go through `core/startup_guard.rs` (`try_acquire_lock`), which polls
`try_lock` until its wait runs out. The lock files stay, empty.

| # | Lock file | Taken in | Wait, then | Purpose |
|---|-----------|----------|------------|---------|
| F1 | `.mcp-startup.lock` | `cli/dispatch/server.rs` | 3 s, then starts without it | Spreads simultaneous MCP server starts |
| F2 | `.startup-maintenance.lock` | `server/server_handler.rs` | 2 s, then skips the pruning | One process prunes old sessions and indexes at a time |
| F3 | `.graph-idx-<hash>.lock` | `core/graph_index/mod.rs` | up to 2 min, then reuses the winner's index when current | One graph scan per project |
| F4 | `.bm25-idx-<hash>.lock` | `core/index_orchestrator.rs` | 800 ms, then loads the shared index | One BM25 build per project |
| F5 | `.search-idx-<hash>.lock` | `core/search_index.rs` | 200 ms, then defers the pre-warm | One search-index walk per project |
| F6 | `knowledge/<hash>/.knowledge.lock` | `core/knowledge/persist.rs` | blocks | Knowledge read-modify-write across processes; taken inside L41 |
| F7 | `usage.jsonl` itself | `core/usage/mod.rs` | blocks | One appended line at a time |

F1–F5 wait a bounded time and then go on without the lock, so they cannot deadlock. F6 and F7
block, but only around a short file read-modify-write or append; F6 is the one taken while a
static lock (L41's inner mutex) is held.

---

## 2. Arc-wrapped Session Locks (per-MCP-session, `tokio::sync::RwLock`)

Fields of `SobreServer` in `tools/server.rs`:

| Field | Type | Purpose |
|-------|------|---------|
| `cache` | `Arc<RwLock<SessionCache>>` | File content cache |
| `session` | `Arc<RwLock<SessionState>>` | Session metadata |
| `last_call` | `Arc<RwLock<Instant>>` | Idle-timeout tracking |
| `client_name` | `Arc<RwLock<String>>` | Connected client name |
| `refetch_signals` | `Arc<RwLock<RefetchSignals>>` | Re-reads, re-runs and expansions that tell compression it went too far |
| `peer` | `Arc<RwLock<Option<Peer<RoleServer>>>>` | MCP client peer handle |
| `roots_gate` | `Arc<tokio::sync::Mutex<()>>` | Serializes the first-call project-root resolution (MCP `roots/list`, then the cwd) so concurrent first calls resolve it once; taken before `peer` and `session`, released once the root is set, never held across a tool call |
| `bm25_cache` | `Arc<std::sync::Mutex<Option<Bm25CacheEntry>>>` | Session BM25 index cache (`core/bm25_cache.rs`, `SharedBm25Cache`); a **`std::sync::Mutex`**, not tokio — never held across `.await` |

Apart from `bm25_cache`, these are **`tokio::sync::RwLock`** and are scoped to a single
session — no cross-session nesting is expected. Tool handlers receive `cache`, `session` and
`bm25_cache` through `ToolContext` (`server/tool_trait.rs`).

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

L17 lives in the shared `core::path_locks` registry (`per_file_lock`) and is used by
`sobre_read` (`tools/registered/sobre_read.rs`), by `sobre_patch`
(`tools/registered/sobre_patch.rs`: the anchored ops in `tools/sobre_patch/`, `replace_unique` and
`replace_all` through the string-replacement engine in `tools/impl_edit.rs`), and by the symbol
edits of `sobre_refactor` and `sobre_patch op=replace_symbol` (`lsp/edit_apply.rs`). The outer
`Mutex<HashMap>` is held briefly to clone the per-path `Arc<Mutex<()>>`, then dropped before the
per-file lock is acquired. The per-file lock is acquired before the session cache lock. This
serializes concurrent operations on the *same* path; operations on different files proceed
independently.

**Edit path (`sobre_patch`, `locked_edit`):** acquires the L17 per-file lock (bounded
`try_lock()` loop, 30s deadline) and then performs **all** disk I/O — read preimage, apply,
TOCTOU recheck, atomic rename — *without* holding the session cache lock. The cache lock is
taken only twice, each briefly and with a 5s timeout: a shared `read()` to fetch the recorded
read mode before the I/O, and an exclusive `write()` to apply the deferred `CacheEffect`
(invalidate / store-full) after the I/O. After a write, the session lock is taken (5s timeout)
to mark the file modified. Same-file edit correctness is guaranteed by the TOCTOU preimage
guard plus the atomic temp-file rename inside `run_io`, not by the cache lock.

**Symbol edits (`lsp/edit_apply.rs`):** hold the per-file lock with a blocking `lock()` for the
read-check-write of one range; no cache or session lock is taken there.

**Read path (`sobre_read`, `read_through_cache`):** before it, the session lock is read
(`try_read_owned()` loop, 8s deadline) for the session's task and released. Then, on the
blocking pool where the handler runs:

1. **Per-file lock:** `try_lock()` loop, 25s deadline.
2. **Shared cache lock:** try the `[unchanged]` stub under `cache.try_read()`; on a hit, return
   without taking the write lock.
3. **Cache write lock (10s deadline):** `prepare` — cache lookup, the disk read and every mode
   that needs no rendering.
4. **No cache lock:** render a compressed view (parsing, compression).
5. **Cache write lock (5s deadline):** store the result and metadata; on timeout the result is
   returned without caching.

After the per-file and cache locks are released, the session lock is taken for writing (10s
timeout) to record the read. The bounce tracker (L38) is advanced before the cache lock is taken
and the read recorded after it is released; the batch read (`registered/batch_read.rs`) still
takes L38 under the cache write lock, which is safe because L38 is a leaf lock.

```
L17 outer (FILE_LOCKS map)        — held briefly to clone Arc, then dropped
 └─► L17 inner (per-file Mutex)   — try_lock() with deadline
      ├─► cache.try_read()        — stub hit? → return early
      ├─► cache write (≤10s)      — prepare: lookup, disk read
      ├─► render                  — NO cache lock
      └─► cache write (≤5s)       — store
```

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

Session-scoped locks on `SobreServer` are logically independent:

- Acquire at most **one session lock per tool handler** at a time, except the sequences
  documented above (per-file lock → cache lock; cache lock released → session lock in the edit
  path).
- If you must hold two, acquire in field-declaration order (cache → session → …).
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
